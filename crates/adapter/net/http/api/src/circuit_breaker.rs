//! The `CircuitBreaker` resilience layer (ADR-0031 §5): the reactive 429/outage
//! backstop to `RateLimit`'s proactive pacing.
//!
//! `RateLimit` tries never to hit a 429; `CircuitBreaker` stops cold if the host
//! fails anyway. It trips **Open** after [`CircuitBreakerConfig::failure_threshold`]
//! consecutive transport failures (`HttpError::{Connection, Timeout}` or a `5xx`
//! response), or **immediately** on a `Throttled`/429 with the long
//! [`CircuitBreakerConfig::throttle_cooldown`] (IBKR's ~15-minute penalty box).
//! While Open it **fast-rejects** every request with a non-retryable
//! [`HttpError::CircuitOpen`] — the inner stack is
//! never touched. After the cooldown a bounded number of **Half-Open** probes test
//! recovery: a reached-host response closes the circuit, a failure re-opens it.
//!
//! The state machine lives in a pure, clock-injected `Breaker` (transitions take
//! `now: Instant` as an input, table-tested with zero async); the `CircuitBreaker`
//! service is a thin `Arc<Mutex<Breaker>>` + [`Timer`](oath_adapter_net_api::Timer)
//! shell. A **single per-host** breaker is shared behind `Arc`. Runtime-neutral and
//! `now()`-only — the breaker never sleeps (Open→Half-Open is a lazy comparison on
//! the next admit), so there is no timer race and no new dependency. Body-transparent
//! — `http::Response<B>` is forwarded untouched.

use crate::HttpError;
use oath_adapter_net_api::{ErrorKind, HasErrorKind};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// The circuit breaker's thresholds, as plain `Copy` data (ADR-0031 §5).
///
/// `failure_threshold` and `half_open_probes` are `NonZeroU32`: "≥ 1" is a type
/// invariant, so `CircuitBreakerLayer::new` needs no
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
#[allow(dead_code)]
pub(crate) enum Class {
    /// A genuine transport/server failure — advances the Closed trip counter.
    Failure,
    /// A throttle/429 — trips the circuit **immediately** on the long cooldown.
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
/// kind, and `5xx` responses are all `Failure`; `Throttled`/429 is `TripNow`; a
/// `4xx`/`Auth`/unclassified error is `Ignored` (never trips **and never
/// resets** — so an interleave cannot mask a building outage); `2xx`/`3xx` is
/// `Success`. `Unknown → Ignored` is the conservative v1 default (the
/// resilience4j fail-safe `Unknown → Failure` is a documented future
/// improvement).
#[allow(dead_code)]
pub(crate) fn classify<B>(outcome: &Result<http::Response<B>, HttpError>) -> Class {
    match outcome {
        Err(e) => match e.kind() {
            // Server (5xx-equivalent error kind) grouped with transport failures —
            // defensive: no HttpError maps here today, but keeps classify total if
            // kind() widens.
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Server => Class::Failure,
            ErrorKind::Throttled => Class::TripNow,
            // Auth, Client, Unknown, CircuitOpen — and any future kind — are Ignored
            // (no HttpError maps to Client either today; same defensive rationale).
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) struct Breaker {
    state: BreakerState,
    cfg: CircuitBreakerConfig,
}

#[allow(dead_code)]
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
    fn throttle_and_429_trip_now() {
        assert_eq!(classify::<()>(&Err(HttpError::Throttled)), Class::TripNow);
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
}
