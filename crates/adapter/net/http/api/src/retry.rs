//! The `Retry` resilience layer (ADR-0031 §2): order-safe retry.
//!
//! Re-issues an **explicitly-eligible** request (a [`Retryable`] marker
//! extension — **absent → never retried**, so a forgotten stamp never
//! duplicates a `POST`) on a **transient** failure (`HttpError::{Timeout,
//! Connection}`) or a `5xx` response, with capped-exponential **full-jitter**
//! backoff up to [`RetryConfig::max_attempts`]. A 429/other 4xx, an `Auth`
//! error, or an `Other` error is **never** retried; on exhaustion the last
//! outcome is returned verbatim. **Body-transparent:** the response body is
//! returned untouched (a superseded response is dropped, releasing any
//! `Guarded` permit). `Auth`/`RateLimit` re-run per attempt because they sit
//! *inside* `Retry`. Runtime-neutral: generic over
//! [`Timer`](oath_adapter_net_api::Timer), jitter via an internal seeded
//! `SplitMix64` (no `rand` dependency).

use std::num::NonZeroU32;
use std::time::Duration;

/// A marker `http::Request` extension: its **presence** opts the request into
/// retry (ADR-0031 §2). `Copy` so it survives the per-attempt request clone.
///
/// Eligibility is **explicit-only and fail-safe**: an **absent** marker means
/// the request is sent exactly once and its outcome returned verbatim — a
/// forgotten stamp disables retry, it never duplicates a non-idempotent `POST`.
/// This tightens ADR-0031 §2's "retry idempotent *methods*" default into
/// adapter-stamped intent, the same structural-safety move ADR-0034 Amendment #1
/// made for `RateScope` (see ADR-0034 Amendment #8).
#[derive(Debug, Clone, Copy)]
pub struct Retryable;

/// The `Retry` layer's schedule, as plain `Copy` data.
///
/// `max_attempts` is the **total** number of sends (retries = `max_attempts − 1`);
/// `NonZeroU32` makes "at least one send" a type invariant, so
/// `RetryLayer::new` needs no `Result`. Backoff before the
/// `n`-th retry draws a full-jitter delay from `[0, min(cap, base·2ⁿ⁻¹)]`; `seed`
/// seeds the jitter PRNG (varied per process in production, fixed in tests).
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Total sends allowed for one logical request (retries = this − 1).
    pub max_attempts: NonZeroU32,
    /// The first backoff ceiling — the `n = 1` retry draws its delay from `[0, base]`.
    pub base: Duration,
    /// The exponential-ceiling clamp — no backoff ceiling exceeds this.
    pub cap: Duration,
    /// The jitter PRNG seed (deterministic given seed + draw order).
    pub seed: u64,
}

#[cfg(test)]
mod tests {
    use super::Retryable;

    #[test]
    fn retryable_marker_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut().insert(Retryable);
        assert!(
            req.extensions().get::<Retryable>().is_some(),
            "marker present → eligible"
        );

        let bare = http::Request::new(bytes::Bytes::new());
        assert!(
            bare.extensions().get::<Retryable>().is_none(),
            "absent marker → not eligible (fail-safe)"
        );
    }
}
