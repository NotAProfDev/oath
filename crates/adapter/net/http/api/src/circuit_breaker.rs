//! The `CircuitBreaker` resilience layer (ADR-0031 §5): the reactive 429/outage
//! backstop to `RateLimit`'s proactive pacing.
//!
//! `RateLimit` tries never to hit a 429; `CircuitBreaker` stops cold if the host
//! fails anyway. It trips **Open** after [`CircuitBreakerConfig::failure_threshold`]
//! consecutive transport failures (`HttpError::{Connection, Timeout}` or a `5xx`
//! response), or **immediately** on a venue **429 response** with the long
//! [`CircuitBreakerConfig::throttle_cooldown`] (IBKR's ~15-minute penalty box). A
//! `Throttled` *error* is a local pacing decision (the request was never sent) and
//! is ignored — it never trips the breaker.
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

use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The circuit breaker's thresholds, as plain `Copy` data (ADR-0031 §5).
///
/// `failure_threshold` and `half_open_probes` are `NonZeroU32`: "≥ 1" is a type
/// invariant, so [`CircuitBreakerLayer::new`] needs no
/// `Result` (a `0` threshold is nonsense and `0` probes would leave a tripped
/// circuit stuck Open forever). This types §5's `u32` sketch more precisely.
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures in the Closed state that trip the circuit Open.
    pub failure_threshold: NonZeroU32,
    /// The cooldown before Half-Open probing after a failure-threshold trip.
    pub cooldown: Duration,
    /// The (longer) cooldown after a `Throttled`/429 trip — the penalty box.
    pub throttle_cooldown: Duration,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState {
    /// Passing requests; `consecutive_failures` counts toward the trip threshold.
    Closed { consecutive_failures: u32 },
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
    /// Admit the call to the inner stack.
    Pass,
    /// Reject the call fast with `CircuitOpen` — the inner stack is not touched.
    Reject,
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
    /// A fresh breaker starts Closed with no failures.
    pub(crate) const fn new(cfg: CircuitBreakerConfig) -> Self {
        Self {
            state: BreakerState::Closed {
                consecutive_failures: 0,
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
                    Admit::Pass
                } else {
                    Admit::Reject
                }
            },
            BreakerState::HalfOpen { probes_left, .. } => {
                if *probes_left > 0 {
                    *probes_left -= 1;
                    Admit::Pass
                } else {
                    Admit::Reject // concurrency gate: no more than `half_open_probes` in flight
                }
            },
        }
    }

    /// Record a classified outcome, transitioning as ADR-0031 §5 dictates.
    pub(crate) fn record(&mut self, class: Class, now: Instant) {
        match self.state {
            BreakerState::Closed {
                consecutive_failures,
            } => match class {
                Class::Failure => {
                    let n = consecutive_failures.saturating_add(1);
                    self.state = if n >= self.cfg.failure_threshold.get() {
                        BreakerState::Open {
                            reopen_at: now + self.cfg.cooldown,
                        }
                    } else {
                        BreakerState::Closed {
                            consecutive_failures: n,
                        }
                    };
                },
                Class::TripNow => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.throttle_cooldown,
                    };
                },
                Class::Ignored => {}, // streak untouched — a 4xx/Auth neither trips nor resets
                Class::Success => {
                    self.state = BreakerState::Closed {
                        consecutive_failures: 0,
                    };
                },
            },
            BreakerState::HalfOpen {
                probes_left,
                successes_needed,
            } => match class {
                Class::Failure => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.cooldown,
                    };
                },
                Class::TripNow => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.throttle_cooldown,
                    };
                },
                // A reached-host probe (2xx/3xx or 4xx/Auth) resolves; the last one closes.
                Class::Ignored | Class::Success => {
                    self.state = if successes_needed <= 1 {
                        BreakerState::Closed {
                            consecutive_failures: 0,
                        }
                    } else {
                        BreakerState::HalfOpen {
                            probes_left,
                            successes_needed: successes_needed - 1,
                        }
                    };
                },
            },
            // A stale outcome from a call admitted before a concurrent trip; drop it.
            // Never un-trips a freshly-opened circuit (single global v1 breaker).
            BreakerState::Open { .. } => {},
        }
    }

    /// Resolve a Half-Open probe whose call was **abandoned** (the future was
    /// dropped by caller cancellation, or the inner service panicked) before its
    /// outcome could be recorded. Only meaningful in Half-Open: reopen so the
    /// episode ends and the circuit self-heals after `cooldown` — a probe with an
    /// unknown outcome must not optimistically close. A **no-op** in `Closed` (a
    /// cancelled call is not a host-health signal, so it must not advance the trip
    /// streak) and in `Open` (already tripped). This is what makes "every admitted
    /// probe reaches a decisive resolution" hold even under cancellation.
    pub(crate) fn on_abandoned_probe(&mut self, now: Instant) {
        if matches!(self.state, BreakerState::HalfOpen { .. }) {
            self.state = BreakerState::Open {
                reopen_at: now + self.cfg.cooldown,
            };
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
    type Service = CircuitBreaker<S, T>;

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
            let mut breaker = self
                .breaker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            breaker.on_abandoned_probe(now);
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
            let admit = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                breaker.admit(now)
            };
            if admit == Admit::Reject {
                return Err(HttpError::CircuitOpen); // fast reject — the leaf is not touched
            }

            // Arm the drop-guard: if this future is cancelled (or the leaf panics)
            // before the real outcome is recorded below, the guard resolves the
            // (possibly Half-Open) probe instead of stranding the breaker.
            let mut guard = ProbeGuard::arm(&self.breaker, &self.timer);
            let outcome = self.inner.call(req).await; // NO lock held across the await
            guard.disarm(); // the future was NOT cancelled — record the true outcome below

            // Record the classified outcome under a second short lock.
            let class = classify(&outcome);
            {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                breaker.record(class, now);
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

    fn cfg(threshold: u32, probes: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown: Duration::from_secs(30),
            throttle_cooldown: Duration::from_secs(900),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }

    #[test]
    fn closed_trips_after_threshold_consecutive_failures() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        assert_eq!(b.admit(now), Admit::Pass);
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Pass, "still closed after 2 failures");
        b.record(Class::Failure, now);
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "3rd consecutive failure → Open rejects"
        );
    }

    #[test]
    fn a_success_resets_the_failure_streak() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        b.record(Class::Success, now); // reset
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Pass, "streak reset → not tripped");
    }

    #[test]
    fn ignored_does_not_reset_the_streak() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::Failure, now);
        b.record(Class::Ignored, now); // a 4xx does NOT reset — anti-masking
        b.record(Class::Failure, now);
        b.record(Class::Failure, now); // 3rd failure overall → trips
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "ignored left the streak intact → trips"
        );
    }

    #[test]
    fn throttle_trips_immediately_on_the_long_cooldown() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::TripNow, now); // one throttle → Open, no threshold needed
        assert_eq!(b.admit(now), Admit::Reject);
        assert_eq!(
            b.admit(now + Duration::from_secs(30)),
            Admit::Reject,
            "the short cooldown is insufficient for a throttle trip"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(900)),
            Admit::Pass,
            "throttle_cooldown elapsed → first probe admitted"
        );
    }

    #[test]
    fn open_rejects_until_cooldown_then_admits_one_probe() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // trips on the first failure
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Reject);
        let after = now + Duration::from_secs(30);
        assert_eq!(
            b.admit(after),
            Admit::Pass,
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
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Success, after);
        assert_eq!(b.admit(after), Admit::Pass, "probe succeeded → closed");
    }

    #[test]
    fn half_open_probe_ignored_also_closes() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Ignored, after); // a 4xx probe still proves the host is reachable
        assert_eq!(
            b.admit(after),
            Admit::Pass,
            "ignored probe → closed (no stuck half-open)"
        );
    }

    #[test]
    fn half_open_probe_failure_reopens() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Failure, after); // probe fails → reopen with a fresh cooldown
        assert_eq!(b.admit(after), Admit::Reject, "re-opened");
        assert_eq!(
            b.admit(after + Duration::from_secs(30)),
            Admit::Pass,
            "the fresh cooldown runs from the failed probe"
        );
    }

    #[test]
    fn multi_probe_half_open_requires_all_to_close() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 2)); // 2 probes per episode
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass, "probe 1");
        assert_eq!(b.admit(after), Admit::Pass, "probe 2");
        assert_eq!(b.admit(after), Admit::Reject, "no probe 3 (gate)");
        b.record(Class::Success, after); // 1 of 2
        assert_eq!(
            b.admit(after),
            Admit::Reject,
            "still half-open, awaiting the 2nd"
        );
        b.record(Class::Success, after); // 2 of 2 → close
        assert_eq!(b.admit(after), Admit::Pass, "both probes reached → closed");
    }

    #[test]
    fn abandoned_probe_reopens_half_open() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now); // → Open
        let after = now + Duration::from_secs(30);
        assert_eq!(
            b.admit(after),
            Admit::Pass,
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
            Admit::Pass,
            "self-healed after a fresh cooldown from the abandonment"
        );
    }

    #[test]
    fn abandoned_probe_is_a_noop_in_closed() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::Failure, now); // streak = 1
        b.record(Class::Failure, now); // streak = 2
        b.on_abandoned_probe(now); // must NOT advance the streak
        assert_eq!(
            b.admit(now),
            Admit::Pass,
            "2 real failures < threshold 3 — abandon was a no-op"
        );
        b.record(Class::Failure, now); // the 3rd REAL failure trips it
        assert_eq!(b.admit(now), Admit::Reject, "3rd real failure → tripped");
    }

    #[test]
    fn abandoned_probe_is_a_noop_in_open() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now); // → Open { reopen_at: now + 30s }
        b.on_abandoned_probe(now + Duration::from_secs(5)); // must not push the deadline out
        assert_eq!(
            b.admit(now + Duration::from_secs(29)),
            Admit::Reject,
            "reopen_at unchanged by the no-op abandon"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(30)),
            Admit::Pass,
            "original cooldown still elapses on schedule"
        );
    }

    #[test]
    fn record_while_open_never_untrips() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now); // → Open
        b.record(Class::Success, now); // a stale success from a pre-trip admit
        assert_eq!(
            b.admit(now),
            Admit::Reject,
            "the Open no-op arm must never un-trip a freshly-opened circuit"
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
                }
            }
        }
    }

    fn cfg(
        threshold: u32,
        cooldown: Duration,
        throttle: Duration,
        probes: u32,
    ) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown,
            throttle_cooldown: throttle,
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
        timer.advance(secs(900)); // now past throttle_cooldown
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
}
