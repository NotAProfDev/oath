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
use std::sync::atomic::{AtomicU64, Ordering};
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

/// A small [SplitMix64](https://prng.di.unimi.it/splitmix64.c) PRNG for backoff
/// jitter — deterministic given its seed and draw order.
///
/// Lock-free: the 64-bit state advances by the `SplitMix64` step constant via
/// `AtomicU64::fetch_add`, so `duration_in` takes `&self` and holds **no** lock
/// across the backoff `await` (the future stays `Send`). Not cryptographic —
/// full-jitter backoff needs a spread, not uniformity guarantees.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SplitMix64 {
    state: AtomicU64,
}

impl Clone for SplitMix64 {
    fn clone(&self) -> Self {
        // Snapshot the current state — a cloned service continues the sequence.
        Self {
            state: AtomicU64::new(self.state.load(Ordering::Relaxed)),
        }
    }
}

#[allow(dead_code)]
impl SplitMix64 {
    /// The `SplitMix64` stepping constant (fractional bits of the golden ratio).
    const STEP: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Seed the generator.
    pub(crate) const fn new(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed),
        }
    }

    /// Advance the state and return the next 64-bit draw (`SplitMix64` finalizer).
    fn next_u64(&self) -> u64 {
        // `fetch_add` returns the *old* state; add STEP to get the new one — so a
        // fresh generator's first draw finalizes `seed + STEP`, as the reference does.
        let mut z = self
            .state
            .fetch_add(Self::STEP, Ordering::Relaxed)
            .wrapping_add(Self::STEP);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform `Duration` in `[0, ceil]` — one full-jitter sample.
    pub(crate) fn duration_in(&self, ceil: Duration) -> Duration {
        // `ceil` comes from `backoff_ceiling` (≤ `cap`); clamp its nanos into u64
        // (a `cap` above ~584 years is not a real config — clamp, don't panic).
        let ceil_nanos = u64::try_from(ceil.as_nanos()).unwrap_or(u64::MAX);
        if ceil_nanos == 0 {
            return Duration::ZERO;
        }
        // Uniform in [0, ceil_nanos]. `saturating_add(1)` avoids a `% 0` when
        // ceil_nanos == u64::MAX; modulo bias is irrelevant for backoff jitter.
        let modulus = ceil_nanos.saturating_add(1);
        Duration::from_nanos(self.next_u64() % modulus)
    }
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

#[cfg(test)]
mod rng_tests {
    use super::SplitMix64;
    use std::time::Duration;

    #[test]
    fn same_seed_reproduces_the_same_sequence() {
        let a = SplitMix64::new(0x1234_5678);
        let b = SplitMix64::new(0x1234_5678);
        let ceil = Duration::from_secs(1);
        for _ in 0..64 {
            assert_eq!(
                a.duration_in(ceil),
                b.duration_in(ceil),
                "seeded PRNG is deterministic"
            );
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let a = SplitMix64::new(1);
        let b = SplitMix64::new(2);
        let ceil = Duration::from_secs(1);
        // Over many draws the two sequences must differ somewhere (not lockstep).
        let differs = (0..64).any(|_| a.duration_in(ceil) != b.duration_in(ceil));
        assert!(
            differs,
            "different seeds must not produce identical sequences"
        );
    }

    #[test]
    fn draws_never_exceed_the_ceiling() {
        let rng = SplitMix64::new(42);
        let ceil = Duration::from_micros(500);
        for _ in 0..10_000 {
            assert!(
                rng.duration_in(ceil) <= ceil,
                "full jitter stays within [0, ceil]"
            );
        }
    }

    #[test]
    fn zero_ceiling_yields_zero() {
        let rng = SplitMix64::new(7);
        assert_eq!(rng.duration_in(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn clone_snapshots_state_independently() {
        let a = SplitMix64::new(99);
        let ceil = Duration::from_millis(50);
        let _ = a.duration_in(ceil); // advance `a`
        let b = a.clone(); // `b` continues from `a`'s current state
        assert_eq!(
            a.duration_in(ceil),
            b.duration_in(ceil),
            "clone snapshots the state"
        );
    }
}
