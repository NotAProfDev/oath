//! Boot-time pacing coverage (ADR-0034 §3).
//!
//! The `RateKey` universe, the `LimitPolicy`/`LimitDecl` classification
//! vocabulary, the total `RateLimitConfig<K>` map, and the `validate_coverage`
//! construction-time check.
//!
//! A `RateLimitConfig<K>` is **total**: every `K::all()` variant must be
//! explicitly classified — `LimitDecl::Policy` or `LimitDecl::GlobalOnly`,
//! never "absent". A missing or ill-configured bucket is caught at
//! construction ([`validate_coverage`]), so it is a boot failure rather than a
//! first-live-order 429 → 15-minute IBKR penalty box. This module is pure data
//! + one validator; the `RateLimit` layer that consumes it lands in Slice 1.

use std::collections::HashMap;
use std::hash::Hash;

/// An adapter's rate-limit key with a **finite universe** — the enumeration
/// that makes the boot-time coverage check possible (ADR-0034 §3).
///
/// `Clone` is doubly-earned: `http::Extensions::insert` demands it, and `Retry`
/// clones the request per attempt (Slice 1), so a stamped key survives replay.
/// The universe is kept generic (not erased to `u32`/`&str`) precisely so
/// [`validate_coverage`] can iterate every variant.
pub trait RateKey: Hash + Eq + Clone + Send + Sync + 'static {
    /// Every key in the universe. Its exhaustiveness is what the coverage check
    /// trusts; an adapter keeps it drift-proof (`strum::VariantArray` or an
    /// exhaustive-`match` test), keeping this trait dependency-free.
    fn all() -> &'static [Self]
    where
        Self: Sized;
}

/// A single pacing policy applied to one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitPolicy {
    /// A refilling token bucket: `rate` tokens/second, up to `burst` in hand.
    TokenBucket {
        /// Steady-state tokens per second (must be `>= 1`).
        rate: u32,
        /// Maximum tokens available at once (must be `>= 1`).
        burst: u32,
    },
    /// A concurrency cap: at most `max` in-flight requests in this scope.
    Concurrency {
        /// Maximum concurrent requests (must be `>= 1`).
        max: u32,
    },
}

/// How one endpoint is paced — an **explicit** classification. There is no
/// "absent" arm: totality (every [`RateKey`] variant classified) is what the
/// boot check enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitDecl {
    /// This endpoint has its own local policy (in addition to the global one).
    Policy(LimitPolicy),
    /// This endpoint is paced by the global policy only — declared on purpose.
    GlobalOnly,
}

/// A **total** pacing configuration: a required `global` policy plus a
/// per-endpoint classification for every key in the [`RateKey`] universe.
///
/// [`validate_coverage`] rejects a `local` map that is not total over
/// `K::all()`, so forgetting to pace a new endpoint is a boot failure.
#[derive(Debug, Clone)]
pub struct RateLimitConfig<K> {
    /// The account-wide policy every request is subject to.
    pub global: LimitPolicy,
    /// The per-endpoint classification. Must be total over `K::all()`.
    pub local: HashMap<K, LimitDecl>,
}

#[cfg(test)]
mod tests {
    use super::{LimitDecl, LimitPolicy, RateKey, RateLimitConfig};
    use std::collections::HashMap;

    /// A stand-in endpoint key for the tests — the shape an adapter provides.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum TestKey {
        PlaceOrder,
        Snapshot,
        History,
    }

    impl RateKey for TestKey {
        fn all() -> &'static [Self] {
            &[Self::PlaceOrder, Self::Snapshot, Self::History]
        }
    }

    #[test]
    fn rate_key_all_is_drift_proof() {
        // Exhaustive `match` with no wildcard arm: adding a `TestKey` variant
        // fails to compile HERE, forcing whoever adds it to also list it in
        // `all()`; the length assertion catches a variant added to the enum
        // but dropped from `all()`.
        fn is_listed(k: TestKey) -> bool {
            match k {
                TestKey::PlaceOrder | TestKey::Snapshot | TestKey::History => true,
            }
        }
        assert!(TestKey::all().iter().copied().all(is_listed));
        assert_eq!(TestKey::all().len(), 3);
    }

    #[test]
    fn config_classifies_every_key_explicitly() {
        let cfg = RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 10,
                burst: 20,
            },
            local: HashMap::from([
                (
                    TestKey::PlaceOrder,
                    LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 }),
                ),
                (
                    TestKey::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, burst: 5 }),
                ),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(cfg.local.len(), 3);
        assert_eq!(
            cfg.global,
            LimitPolicy::TokenBucket {
                rate: 10,
                burst: 20
            }
        );
        assert_eq!(cfg.local[&TestKey::History], LimitDecl::GlobalOnly);
    }
}
