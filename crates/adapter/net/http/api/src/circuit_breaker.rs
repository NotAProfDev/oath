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
use std::time::Duration;

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
