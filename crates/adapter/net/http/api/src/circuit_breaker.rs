//! The `CircuitBreaker` resilience layer (ADR-0031 §5, Amendment #3): the reactive
//! 429/outage backstop to `RateLimit`'s proactive pacing.
//!
//! `RateLimit` tries never to hit a 429; `CircuitBreaker` stops cold if the host
//! fails anyway. It trips **Open** when the **failure rate** over the last
//! [`CircuitBreakerConfig::window_size`] outcomes reaches
//! [`CircuitBreakerConfig::failure_rate_threshold`] (once at least
//! [`CircuitBreakerConfig::minimum_calls`] samples are present), or **immediately** on a
//! venue **429 response** with the long [`CircuitBreakerConfig::retry_after_fallback`]
//! (IBKR's ~15-minute penalty box). A `Throttled` *error* and a `4xx`/`Auth` are local
//! or client-side and never enter the window.
//! While Open it **fast-rejects** every request with a non-retryable
//! [`HttpError::CircuitOpen`] — the inner stack is
//! never touched. After the cooldown a bounded number of **Half-Open** probes test
//! recovery: a reached-host response closes the circuit, a failure re-opens it.
//!
//! The state machine lives in a pure, clock-injected `Breaker` (transitions take
//! `now: Instant` as an input, table-tested with zero async); the `CircuitBreaker`
//! service is a thin `Arc<Mutex<Breaker>>` + [`Timer`]
//! shell. A **single per-host** breaker is shared behind `Arc`. Runtime-neutral and
//! `now()`-only — the breaker never sleeps (Open→Half-Open is a lazy comparison on
//! the next admit), so there is no timer race and no new dependency. Body-transparent
//! — `http::Response<B>` is forwarded untouched.

use crate::clock::deadline;
use crate::rate_window::{Outcome, RateWindow};
use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The circuit breaker's thresholds, as plain `Copy` data (ADR-0031 §5, Amendment #3).
///
/// `window_size`, `minimum_calls`, and `half_open_probes` are `NonZeroU32` ("≥ 1" is a
/// type invariant); `failure_rate_threshold` and the `minimum_calls ≤ window_size`
/// relationship are validated at boot by `stack::validate_config`.
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Failure-rate percentage (`1..=100`) that trips the circuit over the rolling
    /// window; a 50 % host trips at `50`. Validated at boot.
    pub failure_rate_threshold: u8,
    /// Rolling window size: the last-N outcomes the failure rate is computed over.
    pub window_size: NonZeroU32,
    /// Minimum window samples before the rate can trip (cold-start floor). Must be
    /// `≤ window_size` (validated at boot).
    pub minimum_calls: NonZeroU32,
    /// The cooldown before Half-Open probing after a **rate** trip.
    pub cooldown: Duration,
    /// The `429` reopen wait when the response carries no usable `Retry-After`
    /// (the penalty-box fallback; ≈ 10–15 min for IBKR) — Amendment #2.
    pub retry_after_fallback: Duration,
    /// Ceiling on an honored `429` `Retry-After`: `reopen = min(retry_after, cap)` —
    /// Amendment #2.
    pub retry_after_cap: Duration,
    /// Probes admitted per Half-Open episode; all must reach the host to close.
    pub half_open_probes: NonZeroU32,
}

/// The breaker-relevant classification of one call outcome (pure, state-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// A genuine transport/server failure — advances the Closed trip counter.
    Failure,
    /// A venue **429 response** — trips the circuit **immediately** on the long
    /// cooldown. (A `Throttled` *error* is a local decision and never reaches here.)
    TripNow,
    /// Neither a failure nor a trip (4xx, `Auth`, unclassified) — a no-op in Closed;
    /// resolves a Half-Open probe (a reached host proves recovery).
    Ignored,
    /// A healthy `2xx`/`3xx` response — resets the streak / resolves a probe.
    Success,
}

/// Classify a call outcome for the breaker (ADR-0031 §5).
///
/// Genuine transport failures (`Connection`/`Timeout`), the error-side `Server`
/// kind, and `5xx` responses are all `Failure`; a venue **429 response** is
/// `TripNow`; a `4xx`/`Auth`/`Throttled`/unclassified **error** is `Ignored` — a
/// `Throttled` error is a local pacing decision that never reached the host
/// (ADR-0031 §5), and `Ignored` never trips **and never resets**, so an interleave
/// cannot mask a building outage; `2xx`/`3xx` is `Success`. `Unknown → Ignored` is
/// the conservative v1 default (the resilience4j fail-safe `Unknown → Failure` is a
/// documented future improvement).
pub(crate) fn classify<B>(outcome: &Result<http::Response<B>, HttpError>) -> Class {
    match outcome {
        Err(e) => match e.kind() {
            // Server (5xx-equivalent error kind) grouped with transport failures —
            // defensive: no HttpError maps here today, but keeps classify total if
            // kind() widens.
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Server => Class::Failure,
            // A `Throttled` *error* is a purely LOCAL pacing decision (RateLimit's
            // max_wait breach / fail-closed reject) — the request never reached the
            // host, so it carries zero host-health signal and must NOT trip the
            // breaker. Only a real venue 429 *response* (the Ok-side arm) trips
            // (ADR-0031 §5, clarified). Throttled/Auth/Client/Unknown/CircuitOpen —
            // and any future kind — are therefore Ignored.
            _ => Class::Ignored,
        },
        Ok(resp) => {
            let s = resp.status();
            if s == http::StatusCode::TOO_MANY_REQUESTS {
                Class::TripNow
            } else if s.is_server_error() {
                Class::Failure
            } else if s.is_client_error() {
                Class::Ignored
            } else {
                Class::Success
            }
        },
    }
}

/// The breaker's state (ADR-0031 §5). `Instant` deadlines are compared against
/// `Timer::now()` by the async shell — the core itself never reads a clock.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BreakerState {
    /// Passing requests; `window` accumulates outcomes toward the rate trip.
    Closed { window: RateWindow },
    /// Rejecting fast until `reopen_at`; then the next admit begins Half-Open.
    Open { reopen_at: Instant },
    /// Probing: `probes_left` may still be admitted, `successes_needed` must reach
    /// the host before the circuit closes.
    HalfOpen {
        probes_left: u32,
        successes_needed: u32,
    },
}

/// The admission verdict for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Admit a normal (Closed-state) call to the inner stack.
    Pass,
    /// Admit a **Half-Open probe** — the call whose outcome resolves the episode.
    /// The service arms its cancellation guard only for this verdict, so a
    /// cancelled non-probe call cannot reopen a concurrent Half-Open episode.
    Probe,
    /// Reject the call fast with `CircuitOpen` — the inner stack is not touched.
    Reject,
}

/// A label-only view of the breaker's phase for telemetry (deep review §2C) — the
/// discriminant of [`BreakerState`] without its payload, so it is `Copy` and stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Passing traffic.
    Closed,
    /// Fast-rejecting.
    Open,
    /// Probing.
    HalfOpen,
}

impl Phase {
    /// The stable, low-cardinality telemetry label.
    const fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// Why the breaker transitioned to `Open` — a low-cardinality telemetry reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TripReason {
    /// The rolling failure rate crossed the threshold.
    Rate,
    /// A venue `429` response (`TripNow`) — the penalty box.
    Throttle,
    /// A Half-Open probe failed.
    ProbeFailed,
    /// A Half-Open probe was abandoned (cancelled / panicked).
    Abandoned,
}

impl TripReason {
    /// The stable, low-cardinality telemetry label.
    const fn label(self) -> &'static str {
        match self {
            Self::Rate => "rate",
            Self::Throttle => "throttle",
            Self::ProbeFailed => "probe_failed",
            Self::Abandoned => "abandoned",
        }
    }
}

/// The label for a phase change, or `None` when the phase is unchanged — the shell
/// emits a `http_circuit_breaker_transitions_total{to}` count for each `Some`.
fn transition_label(before: Phase, after: Phase) -> Option<&'static str> {
    (before != after).then(|| after.label())
}

/// The pure circuit-breaker state machine (ADR-0031 §5).
///
/// Clock-injected: every transition takes `now: Instant` as an input, so the whole
/// unit is table-testable with zero async. The async `CircuitBreaker` shell owns
/// the `Mutex` and the `Timer`; this type holds neither.
#[derive(Debug, Clone)]
pub(crate) struct Breaker {
    state: BreakerState,
    cfg: CircuitBreakerConfig,
}

impl Breaker {
    /// A fresh breaker starts Closed with an empty window.
    pub(crate) fn new(cfg: CircuitBreakerConfig) -> Self {
        Self {
            state: BreakerState::Closed {
                window: RateWindow::new(cfg.window_size),
            },
            cfg,
        }
    }

    /// Decide whether to admit a call now, transitioning Open→Half-Open lazily.
    pub(crate) fn admit(&mut self, now: Instant) -> Admit {
        match &mut self.state {
            BreakerState::Closed { .. } => Admit::Pass,
            BreakerState::Open { reopen_at } => {
                if now >= *reopen_at {
                    // Cooldown elapsed → begin a Half-Open episode; THIS call is the
                    // first probe (so `probes_left` starts one short of the budget).
                    let probes = self.cfg.half_open_probes.get();
                    self.state = BreakerState::HalfOpen {
                        probes_left: probes - 1,
                        successes_needed: probes,
                    };
                    Admit::Probe
                } else {
                    Admit::Reject
                }
            },
            BreakerState::HalfOpen { probes_left, .. } => {
                if *probes_left > 0 {
                    *probes_left -= 1;
                    Admit::Probe
                } else {
                    Admit::Reject // concurrency gate: no more than `half_open_probes` in flight
                }
            },
        }
    }

    /// Record a classified outcome, transitioning as ADR-0031 §5 (Amendment #3) dictates.
    ///
    /// In Closed, `Failure`/`Success` feed the rolling window and a `Failure` trips when
    /// the rate crosses the threshold (with enough samples); `Ignored` is never a sample;
    /// a `429` `TripNow` trips immediately (unchanged). `retry_after` is consulted only in
    /// the `TripNow` arms, clamped to `retry_after_cap`.
    pub(crate) fn record(
        &mut self,
        class: Class,
        now: Instant,
        retry_after: Option<Duration>,
    ) -> Option<TripReason> {
        // Hoist config reads (all `Copy`) so the `&mut self.state` match below borrows only
        // `state`, and compute the next state, applying it after the borrow ends.
        let min_calls = self.cfg.minimum_calls.get();
        let threshold = u32::from(self.cfg.failure_rate_threshold);
        let window_size = self.cfg.window_size;
        let rate_reopen = deadline(now, self.cfg.cooldown);
        let tripnow_reopen = deadline(
            now,
            retry_after.map_or(self.cfg.retry_after_fallback, |ra| {
                ra.min(self.cfg.retry_after_cap)
            }),
        );

        // (next state, trip reason). Reason is Some only when entering Open.
        let (next, reason): (Option<BreakerState>, Option<TripReason>) = match &mut self.state {
            BreakerState::Closed { window } => match class {
                Class::Failure => {
                    window.push(Outcome::Failure);
                    if window.should_trip(min_calls, threshold) {
                        (
                            Some(BreakerState::Open {
                                reopen_at: rate_reopen,
                            }),
                            Some(TripReason::Rate),
                        )
                    } else {
                        (None, None)
                    }
                },
                Class::Success => {
                    window.push(Outcome::Success); // dilutes the rate; no reset cliff
                    (None, None)
                },
                Class::Ignored => (None, None), // a 4xx/Auth is not a host-health sample
                Class::TripNow => (
                    Some(BreakerState::Open {
                        reopen_at: tripnow_reopen,
                    }),
                    Some(TripReason::Throttle),
                ),
            },
            BreakerState::HalfOpen {
                probes_left,
                successes_needed,
            } => match class {
                Class::Failure => (
                    Some(BreakerState::Open {
                        reopen_at: rate_reopen,
                    }),
                    Some(TripReason::ProbeFailed),
                ),
                Class::TripNow => (
                    Some(BreakerState::Open {
                        reopen_at: tripnow_reopen,
                    }),
                    Some(TripReason::Throttle),
                ),
                // A reached-host probe (2xx/3xx or 4xx/Auth) resolves; the last one closes
                // to a fresh window.
                Class::Ignored | Class::Success => (
                    Some(if *successes_needed <= 1 {
                        BreakerState::Closed {
                            window: RateWindow::new(window_size),
                        }
                    } else {
                        BreakerState::HalfOpen {
                            probes_left: *probes_left,
                            successes_needed: *successes_needed - 1,
                        }
                    }),
                    None,
                ),
            },
            // A stale outcome from a call admitted before a concurrent trip; drop it.
            BreakerState::Open { .. } => (None, None),
        };
        if let Some(state) = next {
            self.state = state;
        }
        reason
    }

    /// Resolve a Half-Open probe whose call was **abandoned** (the future was
    /// dropped by caller cancellation, or the inner service panicked) before its
    /// outcome could be recorded. Only meaningful in Half-Open: reopen so the
    /// episode ends and the circuit self-heals after `cooldown` — a probe with an
    /// unknown outcome must not optimistically close. A **no-op** in `Closed` (a
    /// cancelled call is not a host-health signal, so it must not advance the trip
    /// streak) and in `Open` (already tripped). This is what makes "every admitted
    /// probe reaches a decisive resolution" hold even under cancellation.
    pub(crate) fn on_abandoned_probe(&mut self, now: Instant) -> Option<TripReason> {
        if matches!(self.state, BreakerState::HalfOpen { .. }) {
            self.state = BreakerState::Open {
                reopen_at: deadline(now, self.cfg.cooldown),
            };
            Some(TripReason::Abandoned)
        } else {
            None
        }
    }

    /// The current [`Phase`] — a pure, clock-free, label-only view for telemetry.
    pub(crate) const fn phase(&self) -> Phase {
        match self.state {
            BreakerState::Closed { .. } => Phase::Closed,
            BreakerState::Open { .. } => Phase::Open,
            BreakerState::HalfOpen { .. } => Phase::HalfOpen,
        }
    }
}

/// The `CircuitBreaker` [`Layer`] factory: holds the single shared breaker + clock.
///
/// `new` constructs the breaker **once** into an `Arc<Mutex<…>>`; every service it
/// produces (and every clone) shares it — a single per-host breaker (ADR-0031 §5).
pub struct CircuitBreakerLayer<T> {
    breaker: Arc<Mutex<Breaker>>,
    timer: T,
}

impl<T> CircuitBreakerLayer<T> {
    /// Build the layer from thresholds and a [`Timer`] clock.
    ///
    /// **Infallible** — `NonZeroU32` makes the two counts "≥ 1" a type invariant
    /// (contrast `RateLimitLayer::new`, which validates a config map). Not `const`:
    /// it allocates the shared `Arc<Mutex<Breaker>>`.
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::{CircuitBreakerLayer, CircuitBreakerConfig};
    /// use oath_adapter_net_mock::MockTimer;
    /// use std::num::NonZeroU32;
    /// use std::time::Duration;
    ///
    /// let cfg = CircuitBreakerConfig {
    ///     failure_rate_threshold: 50,
    ///     window_size: NonZeroU32::new(50).unwrap(),
    ///     minimum_calls: NonZeroU32::new(10).unwrap(),
    ///     cooldown: Duration::from_secs(30),
    ///     retry_after_fallback: Duration::from_secs(900),
    ///     retry_after_cap: Duration::from_secs(1800),
    ///     half_open_probes: NonZeroU32::new(1).unwrap(),
    /// };
    /// let _layer = CircuitBreakerLayer::new(cfg, MockTimer::new());
    /// ```
    #[must_use]
    pub fn new(cfg: CircuitBreakerConfig, timer: T) -> Self {
        Self {
            breaker: Arc::new(Mutex::new(Breaker::new(cfg))),
            timer,
        }
    }
}

impl<T: Clone> Clone for CircuitBreakerLayer<T> {
    fn clone(&self) -> Self {
        Self {
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for CircuitBreakerLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreakerLayer")
            .finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for CircuitBreakerLayer<T> {
    type Wrapped = CircuitBreaker<S, T>;

    fn layer(&self, inner: S) -> CircuitBreaker<S, T> {
        CircuitBreaker {
            inner,
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

/// The `CircuitBreaker` middleware: fast-rejects while Open, else forwards.
///
/// A thin shell over the pure `Breaker`: it locks briefly to `admit` (using
/// `timer.now()`), releases the lock, runs `inner.call` (or returns `CircuitOpen`),
/// then locks briefly to `record` the classified outcome. The lock is **never**
/// held across the `await`. Body-transparent — `http::Response<B>` is forwarded.
pub struct CircuitBreaker<S, T> {
    inner: S,
    // Concurrency-test note (loom): held only for the brief admit()/record()
    // critical sections in `Service::call` below, and NEVER across an `.await`
    // (see the struct doc above). A loom interleaving model would add little over
    // the clock-injected `Breaker` unit tests. Deferred deliberately (Tier-1
    // PR8/#101); revisit if the lock scope ever grows to span an await or the
    // contention model changes.
    breaker: Arc<Mutex<Breaker>>,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for CircuitBreaker<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

impl<S, T> fmt::Debug for CircuitBreaker<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreaker").finish_non_exhaustive()
    }
}

/// Arms a safety net for an admitted call: if the [`CircuitBreaker::call`] future
/// is dropped (caller cancellation) or the inner service panics **before** the real
/// outcome is recorded, this guard's `Drop` resolves the (possibly Half-Open) probe
/// via [`Breaker::on_abandoned_probe`], so a cancelled probe can never strand the
/// breaker in a permanent Half-Open reject. Disarmed the instant the inner call
/// returns normally, so a completed call records its true outcome instead.
struct ProbeGuard<'a, T: Timer> {
    breaker: &'a std::sync::Mutex<Breaker>,
    timer: &'a T,
    armed: bool,
}

impl<'a, T: Timer> ProbeGuard<'a, T> {
    const fn arm(breaker: &'a std::sync::Mutex<Breaker>, timer: &'a T) -> Self {
        Self {
            breaker,
            timer,
            armed: true,
        }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<T: Timer> Drop for ProbeGuard<'_, T> {
    fn drop(&mut self) {
        if self.armed {
            let now = self.timer.now();
            let (transition, reason) = {
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = breaker.phase();
                let reason = breaker.on_abandoned_probe(now);
                (transition_label(before, breaker.phase()), reason)
            };
            if let Some(to) = transition {
                crate::meter::breaker_transition(to, reason.map(TripReason::label));
            }
        }
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for CircuitBreaker<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
{
    type Response = http::Response<B>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            // Admit decision under a short lock (released at the end of this block).
            // The lazy Open→Half-Open transition is captured here and emitted after
            // the lock drops, so no metric call happens while the breaker is locked.
            let (admit, transition) = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = breaker.phase();
                let admit = breaker.admit(now);
                (admit, transition_label(before, breaker.phase()))
            };
            if let Some(to) = transition {
                crate::meter::breaker_transition(to, None);
            }
            let is_probe = match admit {
                Admit::Reject => return Err(HttpError::CircuitOpen), // fast reject — leaf untouched
                Admit::Probe => true,
                Admit::Pass => false,
            };

            // Arm the drop-guard ONLY for a genuine Half-Open probe: if a probe's
            // future is cancelled (or the leaf panics) before its outcome is
            // recorded below, the guard reopens the episode instead of stranding the
            // breaker. A cancelled Closed-state call carries no probe semantics, so
            // it must not touch a concurrently-entered Half-Open episode (M1).
            let mut guard = is_probe.then(|| ProbeGuard::arm(&self.breaker, &self.timer));
            let outcome = self.inner.call(req).await; // NO lock held across the await
            if let Some(g) = guard.as_mut() {
                g.disarm(); // completed normally — record the true outcome below
            }

            // Record the classified outcome under a second short lock; capture any
            // trip/close/half-open transition and emit it after the lock drops.
            let class = classify(&outcome);
            // Honor a delay-seconds `Retry-After` only on a 429 response (ADR-0031
            // Amendment #2): it sets the reopen deadline, clamped by `retry_after_cap`.
            let retry_after = match &outcome {
                Ok(resp) if resp.status() == http::StatusCode::TOO_MANY_REQUESTS => {
                    crate::retry_after::parse_retry_after(resp.headers())
                },
                _ => None,
            };
            let (transition, reason) = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = breaker.phase();
                let reason = breaker.record(class, now, retry_after);
                (transition_label(before, breaker.phase()), reason)
            };
            if let Some(to) = transition {
                crate::meter::breaker_transition(to, reason.map(TripReason::label));
            }
            if retry_after.is_some() {
                crate::meter::retry_after_honored("breaker");
            }
            outcome
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::{Class, classify};
    use crate::HttpError;

    #[allow(clippy::unnecessary_wraps)]
    fn ok(status: u16) -> Result<http::Response<()>, HttpError> {
        let mut r = http::Response::new(());
        *r.status_mut() = http::StatusCode::from_u16(status).unwrap();
        Ok(r)
    }

    #[test]
    fn transport_errors_and_5xx_are_failures() {
        assert_eq!(classify::<()>(&Err(HttpError::Timeout)), Class::Failure);
        assert_eq!(
            classify::<()>(&Err(HttpError::connection("reset"))),
            Class::Failure
        );
        assert_eq!(classify(&ok(500)), Class::Failure);
        assert_eq!(classify(&ok(503)), Class::Failure);
    }

    #[test]
    fn only_a_429_response_trips_now_not_a_local_throttled_error() {
        // A `Throttled` *error* is produced only locally by RateLimit (max_wait /
        // fail-closed reject) — the request never reached the host, so it carries
        // no host-health signal and must be Ignored, never TripNow (ADR-0031 §5).
        assert_eq!(classify::<()>(&Err(HttpError::Throttled)), Class::Ignored);
        // A real venue 429 *response* still trips.
        assert_eq!(classify(&ok(429)), Class::TripNow);
    }

    #[test]
    fn body_too_large_is_breaker_neutral_not_a_failure() {
        // `BodyTooLarge` is a purely LOCAL size-cap rejection — the request reached
        // the host and a response was flowing, but the cap is a local policy
        // decision, not a host-health signal — so it must be Ignored, never
        // Failure, even if the classify catch-all above is ever narrowed.
        assert_eq!(
            classify::<()>(&Err(HttpError::BodyTooLarge)),
            Class::Ignored
        );
    }

    #[test]
    fn client_errors_auth_and_unknown_are_ignored() {
        assert_eq!(classify(&ok(400)), Class::Ignored);
        assert_eq!(classify(&ok(404)), Class::Ignored);
        assert_eq!(
            classify::<()>(&Err(HttpError::auth("expired"))),
            Class::Ignored
        );
        assert_eq!(
            classify::<()>(&Err(HttpError::other("boom"))),
            Class::Ignored
        );
    }

    #[test]
    fn success_statuses_are_success() {
        assert_eq!(classify(&ok(200)), Class::Success);
        assert_eq!(classify(&ok(301)), Class::Success);
    }
}

#[cfg(test)]
mod breaker_tests {
    use super::{Admit, Breaker, CircuitBreakerConfig, Class};
    use std::num::NonZeroU32;
    use std::time::{Duration, Instant};

    // General rate config for policy-specific tests.
    fn rate_cfg(
        threshold_pct: u8,
        window: u32,
        min_calls: u32,
        probes: u32,
    ) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_rate_threshold: threshold_pct,
            window_size: NonZeroU32::new(window).unwrap(),
            minimum_calls: NonZeroU32::new(min_calls).unwrap(),
            cooldown: Duration::from_secs(30),
            retry_after_fallback: Duration::from_secs(900),
            retry_after_cap: Duration::from_secs(1800),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }

    // A config that trips on the FIRST failure (100% over a 1-sample window) — the analogue
    // of the old `failure_threshold = 1`, for the Open/HalfOpen/probe tests whose behavior
    // is independent of the trip policy.
    fn first(probes: u32) -> CircuitBreakerConfig {
        rate_cfg(100, 1, 1, probes)
    }

    #[test]
    fn closed_trips_when_failure_rate_reaches_threshold() {
        let now = Instant::now();
        // 50% over a 20-window, min_calls 10.
        let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
        for _ in 0..5 {
            b.record(Class::Success, now, None);
            b.record(Class::Failure, now, None);
        } // 5F + 5S = 10 samples, 50%
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "50% over 10 samples meets the 50% threshold"
        );
    }

    #[test]
    fn interleaved_successes_do_not_prevent_a_rate_trip() {
        // The motivating case: consecutive-count never tripped this; the rate window does.
        let now = Instant::now();
        let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
        for _ in 0..10 {
            b.record(Class::Failure, now, None);
            b.record(Class::Success, now, None); // a success no longer resets an alarm
        }
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "sustained 50% degradation trips"
        );
    }

    #[test]
    fn below_min_calls_never_trips() {
        let now = Instant::now();
        let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
        for _ in 0..9 {
            b.record(Class::Failure, now, None); // 100% but only 9 < min 10
        }
        assert_eq!(b.admit(now), Admit::Pass, "under the min-calls floor");
    }

    #[test]
    fn ignored_is_not_a_window_sample() {
        let now = Instant::now();
        let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
        for _ in 0..30 {
            b.record(Class::Ignored, now, None); // a flood of 4xx: neither trips nor counts
        }
        assert_eq!(b.admit(now), Admit::Pass, "4xx never enters the window");
    }

    #[test]
    fn window_resets_to_a_clean_slate_on_close() {
        let now = Instant::now();
        // Trips at 2/2 failures (100% over a 2-sample window).
        let mut b = Breaker::new(rate_cfg(100, 2, 2, 1));
        b.record(Class::Failure, now, None); // [F] — 1 < min 2, no trip
        b.record(Class::Failure, now, None); // [F,F] — 100% of 2 → Open
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe);
        b.record(Class::Success, after, None); // probe closes → FRESH empty window
        // One failure into a fresh window is 1/1 = 100% but only 1 sample < min 2 → no trip.
        // Had the pre-trip [F,F] carried over, this failure would keep it tripping.
        b.record(Class::Failure, after, None);
        assert_eq!(
            b.admit(after),
            Admit::Pass,
            "fresh window after close: one failure is below min_calls, so no trip"
        );
    }

    #[test]
    fn throttle_trips_immediately_on_the_long_cooldown() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::TripNow, now, None); // one throttle → Open, no threshold needed
        assert_eq!(b.admit(now), Admit::Reject);
        assert_eq!(
            b.admit(now + Duration::from_secs(30)),
            Admit::Reject,
            "the short cooldown is insufficient for a throttle trip"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(900)),
            Admit::Probe,
            "retry_after_fallback elapsed → first probe admitted"
        );
    }

    #[test]
    fn open_rejects_until_cooldown_then_admits_one_probe() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1)); // trips on the first failure
        b.record(Class::Failure, now, None);
        assert_eq!(b.admit(now), Admit::Reject);
        let after = now + Duration::from_secs(30);
        assert_eq!(
            b.admit(after),
            Admit::Probe,
            "cooldown elapsed → first probe"
        );
        assert_eq!(
            b.admit(after),
            Admit::Reject,
            "concurrency gate: no 2nd probe"
        );
    }

    #[test]
    fn half_open_probe_success_closes() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe);
        b.record(Class::Success, after, None);
        assert_eq!(b.admit(after), Admit::Pass, "probe succeeded → closed");
    }

    #[test]
    fn half_open_probe_ignored_also_closes() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe);
        b.record(Class::Ignored, after, None); // a 4xx probe still proves the host is reachable
        assert_eq!(
            b.admit(after),
            Admit::Pass,
            "ignored probe → closed (no stuck half-open)"
        );
    }

    #[test]
    fn half_open_probe_failure_reopens() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe);
        b.record(Class::Failure, after, None); // probe fails → reopen with a fresh cooldown
        assert_eq!(b.admit(after), Admit::Reject, "re-opened");
        assert_eq!(
            b.admit(after + Duration::from_secs(30)),
            Admit::Probe,
            "the fresh cooldown runs from the failed probe"
        );
    }

    #[test]
    fn half_open_probe_429_reopens_on_the_long_cooldown() {
        // Trip on a normal failure (short 30s cooldown). At Half-Open, the probe returns
        // a 429 (Class::TripNow) — the breaker must re-open on retry_after_fallback (900s),
        // NOT the 30s cooldown a failing probe would use. Distinguish the two: at
        // reopen+30s still Reject; only at reopen+900s does the next probe admit.
        let now = Instant::now();
        let mut b = Breaker::new(first(1)); // trips on the first failure
        b.record(Class::Failure, now, None); // Closed → Open (30s cooldown)
        let probe_at = now + Duration::from_secs(30);
        assert_eq!(b.admit(probe_at), Admit::Probe, "cooldown elapsed → probe");
        b.record(Class::TripNow, probe_at, None); // 429 during the probe → long box
        assert_eq!(
            b.admit(probe_at + Duration::from_secs(30)),
            Admit::Reject,
            "short cooldown is NOT enough after a 429 re-trip"
        );
        assert_eq!(
            b.admit(probe_at + Duration::from_secs(900)),
            Admit::Probe,
            "only retry_after_fallback reopens after a Half-Open 429 re-trip"
        );
    }

    #[test]
    fn multi_probe_half_open_requires_all_to_close() {
        let now = Instant::now();
        let mut b = Breaker::new(first(2)); // 2 probes per episode
        b.record(Class::Failure, now, None);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe, "probe 1");
        assert_eq!(b.admit(after), Admit::Probe, "probe 2");
        assert_eq!(b.admit(after), Admit::Reject, "no probe 3 (gate)");
        b.record(Class::Success, after, None); // 1 of 2
        assert_eq!(
            b.admit(after),
            Admit::Reject,
            "still half-open, awaiting the 2nd"
        );
        b.record(Class::Success, after, None); // 2 of 2 → close
        assert_eq!(b.admit(after), Admit::Pass, "both probes reached → closed");
    }

    #[test]
    fn abandoned_probe_reopens_half_open() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None); // → Open
        let after = now + Duration::from_secs(30);
        assert_eq!(
            b.admit(after),
            Admit::Probe,
            "cooldown elapsed → probe admitted"
        );
        b.on_abandoned_probe(after); // the probe's future was dropped
        assert_eq!(
            b.admit(after),
            Admit::Reject,
            "abandoned probe reopened → still within the fresh cooldown"
        );
        assert_eq!(
            b.admit(after + Duration::from_secs(30)),
            Admit::Probe,
            "self-healed after a fresh cooldown from the abandonment"
        );
    }

    #[test]
    fn abandoned_probe_is_a_noop_in_closed() {
        let now = Instant::now();
        // Trip at 100% over a 3-sample window (min_calls 3).
        let mut b = Breaker::new(rate_cfg(100, 3, 3, 1));
        b.record(Class::Failure, now, None); // 1 sample
        b.record(Class::Failure, now, None); // 2 samples, < min 3 → not tripped
        b.on_abandoned_probe(now); // must NOT add a sample
        assert_eq!(
            b.admit(now),
            Admit::Pass,
            "2 real failures < min_calls 3 — abandon was a no-op"
        );
        b.record(Class::Failure, now, None); // 3rd real failure → 100% of 3 → trips
        assert_eq!(b.admit(now), Admit::Reject, "3rd real failure → tripped");
    }

    #[test]
    fn abandoned_probe_is_a_noop_in_open() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None); // → Open { reopen_at: now + 30s }
        b.on_abandoned_probe(now + Duration::from_secs(5)); // must not push the deadline out
        assert_eq!(
            b.admit(now + Duration::from_secs(29)),
            Admit::Reject,
            "reopen_at unchanged by the no-op abandon"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(30)),
            Admit::Probe,
            "original cooldown still elapses on schedule"
        );
    }

    #[test]
    fn record_while_open_never_untrips() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        b.record(Class::Failure, now, None); // → Open
        b.record(Class::Success, now, None); // a stale success from a pre-trip admit
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "the Open no-op arm must never un-trip a freshly-opened circuit"
        );
    }

    #[test]
    fn admit_distinguishes_a_probe_from_a_normal_pass() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1));
        assert_eq!(
            b.admit(now),
            Admit::Pass,
            "closed → normal pass, not a probe"
        );
        b.record(Class::Failure, now, None); // → Open
        let after = now + Duration::from_secs(30);
        assert_eq!(
            b.admit(after),
            Admit::Probe,
            "half-open admission is a probe"
        );
    }

    #[test]
    fn a_429_retry_after_reopens_on_the_honored_value() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1)); // trips on the first outcome
        // 429 carrying Retry-After: 2 → reopen at now+2s (honored), under the 900s
        // fallback and the 1800s cap.
        b.record(Class::TripNow, now, Some(Duration::from_secs(2)));
        assert_eq!(
            b.admit(now + Duration::from_secs(1)),
            Admit::Reject,
            "before the honored 2s"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(2)),
            Admit::Probe,
            "honored 2s elapsed → probe"
        );
    }

    #[test]
    fn a_429_retry_after_is_clamped_to_the_cap() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1)); // retry_after_cap = 1800s
        b.record(Class::TripNow, now, Some(Duration::from_secs(100_000))); // absurd
        assert_eq!(
            b.admit(now + Duration::from_secs(1799)),
            Admit::Reject,
            "before the 1800s cap"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(1800)),
            Admit::Probe,
            "clamped to the 1800s cap, not 100_000s"
        );
    }

    #[test]
    fn a_429_without_retry_after_uses_the_fallback() {
        let now = Instant::now();
        let mut b = Breaker::new(first(1)); // retry_after_fallback = 900s
        b.record(Class::TripNow, now, None);
        assert_eq!(
            b.admit(now + Duration::from_secs(899)),
            Admit::Reject,
            "before the 900s fallback"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(900)),
            Admit::Probe,
            "fallback 900s elapsed → probe"
        );
    }
}

#[cfg(test)]
mod service_tests {
    use super::{CircuitBreakerConfig, CircuitBreakerLayer};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use oath_adapter_net_api::{ErrorKind, Layer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // One scripted outcome per attempt. `Copy` so the leaf reads it by index.
    #[derive(Clone, Copy)]
    enum Step {
        Err(ErrorKind),
        Status(u16),
        StatusRetryAfter(u16, u64),
    }

    fn err_of(kind: ErrorKind) -> HttpError {
        match kind {
            ErrorKind::Timeout => HttpError::Timeout,
            ErrorKind::Connection => HttpError::connection("reset"),
            ErrorKind::Throttled => HttpError::Throttled,
            ErrorKind::Auth => HttpError::auth("expired"),
            _ => HttpError::other("boom"),
        }
    }

    // An inline leaf yielding a scripted sequence of outcomes, counting calls. Once
    // the script is exhausted it repeats the last step. Body is `()` — the breaker
    // only reads `status()`, never the body. Inline (not `MockClient`) to avoid the
    // net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
    }
    impl ScriptLeaf {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: Arc::new(steps),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<()>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self
                .steps
                .get(i)
                .copied()
                .unwrap_or_else(|| *self.steps.last().unwrap());
            async move {
                match step {
                    Step::Err(kind) => Err(err_of(kind)),
                    Step::Status(code) => {
                        let mut resp = http::Response::new(());
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    },
                    Step::StatusRetryAfter(code, secs) => {
                        let mut resp = http::Response::new(());
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        resp.headers_mut()
                            .insert(http::header::RETRY_AFTER, http::HeaderValue::from(secs));
                        Ok(resp)
                    },
                }
            }
        }
    }

    fn cfg(
        trip_after: u32, // consecutive failures needed at 100% (window = min_calls = trip_after)
        cooldown: Duration,
        fallback: Duration,
        probes: u32,
    ) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_rate_threshold: 100,
            window_size: NonZeroU32::new(trip_after).unwrap(),
            minimum_calls: NonZeroU32::new(trip_after).unwrap(),
            cooldown,
            retry_after_fallback: fallback,
            retry_after_cap: Duration::from_secs(1800),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn bare_req() -> http::Request<Bytes> {
        http::Request::new(Bytes::new())
    }

    #[tokio::test]
    async fn trips_after_threshold_then_fast_rejects_without_touching_the_leaf() {
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]); // always fails
        let svc = CircuitBreakerLayer::new(cfg(3, secs(30), secs(900), 1), MockTimer::new())
            .layer(leaf.clone());
        for _ in 0..3 {
            let _ = svc.call(bare_req()).await; // 3 consecutive failures trip it
        }
        assert_eq!(leaf.calls(), 3);
        let err = svc.call(bare_req()).await.unwrap_err();
        assert!(matches!(err, HttpError::CircuitOpen));
        assert_eq!(
            leaf.calls(),
            3,
            "an open circuit fast-rejects; the leaf is untouched"
        );
    }

    #[test]
    fn tripping_the_breaker_emits_an_open_transition_metric() {
        use futures_util::FutureExt;
        use metrics_util::debugging::{DebugValue, DebuggingRecorder};
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            // threshold = 1 → the first failure trips Closed→Open. MockTimer + inline
            // leaf resolve synchronously, so `now_or_never` drives the whole call.
            let svc = CircuitBreakerLayer::new(cfg(1, secs(30), secs(900), 1), MockTimer::new())
                .layer(ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]));
            svc.call(bare_req())
                .now_or_never()
                .expect("resolves synchronously under MockTimer")
                .unwrap_err();
        });
        let opened = snap.snapshot().into_vec().into_iter().any(|(k, _, _, v)| {
            k.key().name() == "http_circuit_breaker_transitions_total"
                && k.key()
                    .labels()
                    .any(|l| l.key() == "to" && l.value() == "open")
                && k.key()
                    .labels()
                    .any(|l| l.key() == "reason" && l.value() == "rate")
                && matches!(v, DebugValue::Counter(n) if n >= 1)
        });
        assert!(opened, "a rate trip emits to=open, reason=rate");
    }

    #[tokio::test]
    async fn a_single_429_trips_immediately_on_the_long_cooldown() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::Status(429), Step::Status(200)]);
        let svc = CircuitBreakerLayer::new(cfg(3, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let resp = svc.call(bare_req()).await.expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "one 429 trips the circuit"
        );
        timer.advance(secs(30)); // the SHORT cooldown is not enough for a throttle trip
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
        timer.advance(secs(900)); // now past retry_after_fallback
        let resp = svc
            .call(bare_req())
            .await
            .expect("probe admitted, leaf returns 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(
            leaf.calls(),
            2,
            "one 429 + one probe; the fast-rejects never hit the leaf"
        );
    }

    #[tokio::test]
    async fn a_429_during_a_half_open_probe_reopens_on_the_long_cooldown() {
        let timer = MockTimer::new();
        // fail, fail → Open (30s); probe returns 429 → reopen on retry_after_fallback (900s);
        // final probe returns 200.
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Connection),
            Step::Err(ErrorKind::Connection),
            Step::Status(429),
            Step::Status(200),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await; // fail 1
        let _ = svc.call(bare_req()).await; // fail 2 → Open
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
        timer.advance(secs(30)); // normal cooldown → probe admitted
        let resp = svc.call(bare_req()).await.expect("probe reaches the leaf");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS); // 429 as Ok
        // Re-opened on the LONG cooldown: 30s more is not enough…
        timer.advance(secs(30));
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "a 429 probe re-opened on retry_after_fallback, not the 30s cooldown"
        );
        // …but a further advance to a full 900s from the re-trip admits the next probe.
        timer.advance(secs(870)); // 30 + 870 = 900 total since the 429 re-trip
        let ok = svc
            .call(bare_req())
            .await
            .expect("retry_after_fallback elapsed → probe → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
    }

    #[tokio::test]
    async fn a_429_response_retry_after_reopens_on_the_honored_value() {
        let timer = MockTimer::new();
        // A single 429 carrying Retry-After: 5 trips the breaker; it must reopen at +5s
        // (honored), NOT the 900s fallback. A probe at +5s reaches the leaf → 200.
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(429, 5), Step::Status(200)]);
        let svc =
            CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone()).layer(leaf);
        let resp = svc.call(bare_req()).await.expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        // +4s is short of the honored 5s → still Open, fast-reject (leaf untouched).
        timer.advance(secs(4));
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "before the honored 5s the breaker still rejects"
        );
        // +1s more (total 5s) → probe admitted → reaches the leaf → 200.
        timer.advance(secs(1));
        let ok = svc
            .call(bare_req())
            .await
            .expect("honored 5s elapsed → probe → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
    }

    #[tokio::test]
    async fn recovers_when_the_cooldown_probe_succeeds() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Timeout),
            Step::Err(ErrorKind::Timeout),
            Step::Status(200),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await; // fail 1
        let _ = svc.call(bare_req()).await; // fail 2 → Open
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
        timer.advance(secs(30));
        let ok = svc
            .call(bare_req())
            .await
            .expect("probe hits the leaf → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
        let ok2 = svc
            .call(bare_req())
            .await
            .expect("closed → next call flows");
        assert_eq!(ok2.status(), http::StatusCode::OK);
        assert_eq!(
            leaf.calls(),
            4,
            "2 failures + 2 post-recovery sends; rejects skip the leaf"
        );
    }

    #[tokio::test]
    async fn reopens_when_the_cooldown_probe_fails() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Connection),
            Step::Err(ErrorKind::Connection),
            Step::Status(503),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await;
        let _ = svc.call(bare_req()).await; // Open
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
        timer.advance(secs(30));
        let resp = svc
            .call(bare_req())
            .await
            .expect("probe returns a 503 as Ok");
        assert_eq!(resp.status(), 503);
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "the probe failed → re-opened"
        );
        assert_eq!(leaf.calls(), 3);
    }

    #[tokio::test]
    async fn clones_from_one_layer_share_the_breaker() {
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]);
        let layer = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), MockTimer::new());
        let a = layer.layer(leaf.clone());
        let b = a.clone(); // shares the Arc<Mutex<Breaker>>
        let _ = a.call(bare_req()).await; // fail 1 via A
        let _ = a.call(bare_req()).await; // fail 2 via A → Open
        assert!(
            matches!(
                b.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "clone B observes A's trip (single per-host breaker)"
        );
    }

    // A leaf that fails on its first call (tripping `cfg(threshold=1)`) and then
    // never resolves — models the Half-Open probe call getting cancelled in flight.
    #[derive(Clone)]
    struct FailThenHangLeaf {
        calls: Arc<AtomicUsize>,
    }
    impl FailThenHangLeaf {
        fn new() -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }
    impl Service<http::Request<Bytes>> for FailThenHangLeaf {
        type Response = http::Response<()>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            async move {
                if i == 0 {
                    Err(err_of(ErrorKind::Connection))
                } else {
                    // Models an in-flight request that never returns until cancelled.
                    std::future::pending::<Result<http::Response<()>, HttpError>>().await
                }
            }
        }
    }

    #[tokio::test]
    async fn a_cancelled_half_open_probe_reopens_instead_of_wedging() {
        let timer = MockTimer::new();
        let leaf = FailThenHangLeaf::new();
        let svc =
            CircuitBreakerLayer::new(cfg(1, secs(30), secs(900), 1), timer.clone()).layer(leaf);

        // 1. First call is admitted (Closed) but fails → trips the circuit Open.
        //    The call returns the real transport error, not `CircuitOpen` — the
        //    circuit trips only *after* this outcome is recorded.
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::Connection(_)
        ));
        // Confirm the trip: the very next call fast-rejects.
        let err = svc.call(bare_req()).await.unwrap_err();
        assert!(matches!(err, HttpError::CircuitOpen), "confirms Open");

        // 2. Cooldown elapses.
        timer.advance(secs(30));

        // 3. Poll once: admits the Half-Open probe (state → HalfOpen{probes_left:0})
        //    and parks on the never-resolving leaf call.
        // `Box::pin` (not `std::pin::pin!`) so `drop(fut)` below actually runs the
        // future's destructor early — `pin!`'s backing storage lives in a hidden
        // stack slot until the enclosing scope ends, so dropping its `Pin<&mut _>`
        // handle would NOT run `ProbeGuard::drop` at the point we need it to.
        let mut fut = Box::pin(svc.call(bare_req()));
        assert!(
            futures_util::poll!(fut.as_mut()).is_pending(),
            "the probe is admitted and parked on the hanging leaf"
        );

        // 4. Drop the parked future — simulates caller cancellation. `ProbeGuard::drop`
        //    must fire `Breaker::on_abandoned_probe`, reopening the circuit.
        drop(fut);

        // 5. Self-heal, not a wedge: still within the fresh cooldown → fast-reject.
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "reopened with a fresh cooldown from the abandonment"
        );
        // After a fresh cooldown, a new probe is admitted again (parks on the leaf,
        // i.e. polls Pending) instead of being permanently rejected as CircuitOpen.
        timer.advance(secs(30));
        let mut fut2 = std::pin::pin!(svc.call(bare_req()));
        assert!(
            futures_util::poll!(fut2.as_mut()).is_pending(),
            "self-healed: a fresh probe is admitted rather than wedged at probes_left:0"
        );
    }

    // Call 0 hangs (the Closed-era call we cancel), call 1 fails (trips the
    // breaker), call 2 hangs (the live Half-Open probe), call 3+ succeed.
    #[derive(Clone)]
    struct RaceLeaf {
        calls: Arc<AtomicUsize>,
    }
    impl RaceLeaf {
        fn new() -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }
    impl Service<http::Request<Bytes>> for RaceLeaf {
        type Response = http::Response<()>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            async move {
                match i {
                    1 => Err(err_of(ErrorKind::Connection)),
                    0 | 2 => std::future::pending::<Result<http::Response<()>, HttpError>>().await,
                    _ => Ok(http::Response::new(())),
                }
            }
        }
    }

    #[tokio::test]
    async fn a_cancelled_non_probe_call_does_not_reopen_a_concurrent_half_open() {
        let timer = MockTimer::new();
        let leaf = RaceLeaf::new();
        let svc =
            CircuitBreakerLayer::new(cfg(1, secs(30), secs(900), 1), timer.clone()).layer(leaf);

        // 1. A Closed-state call is admitted and parks (call 0 hangs). It is NOT a
        //    probe, so its guard must never be armed. `Box::pin` so `drop` runs the
        //    future's destructor at the point we choose.
        let mut closed_call = Box::pin(svc.call(bare_req()));
        assert!(
            futures_util::poll!(closed_call.as_mut()).is_pending(),
            "the Closed-state call is admitted and parks on the hanging leaf"
        );

        // 2. A second call fails, tripping the breaker Open (call 1).
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::Connection(_)
        ));

        // 3. Cooldown elapses; the real probe is admitted and parks (call 2 hangs) →
        //    Half-Open with the single probe budget spent.
        timer.advance(secs(30));
        let mut probe = Box::pin(svc.call(bare_req()));
        assert!(
            futures_util::poll!(probe.as_mut()).is_pending(),
            "the probe is admitted and parks on the hanging leaf"
        );

        // 4. Cancel the Closed-era call. With probe-only guarding its guard was never
        //    armed, so this must NOT reopen the live Half-Open episode (M1).
        drop(closed_call);

        // 5. Even after a full cooldown the episode is intact (Half-Open, probe budget
        //    spent) → still fast-rejects. Had the drop reopened the circuit (the bug),
        //    the fresh cooldown would have elapsed and this call would reach the leaf
        //    (call 3 → Ok) instead of being rejected.
        timer.advance(secs(30));
        assert!(
            matches!(
                svc.call(bare_req()).await.unwrap_err(),
                HttpError::CircuitOpen
            ),
            "a cancelled non-probe call must not reopen the live Half-Open episode"
        );
    }
}
