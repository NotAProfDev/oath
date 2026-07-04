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
use std::fmt;
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

impl LimitPolicy {
    /// Reject non-sensical policy parameters (ADR-0034 §3 / spec: `rate == 0`,
    /// `burst == 0`, `max == 0`).
    fn validate(self) -> Result<(), BuildError> {
        match self {
            Self::TokenBucket { rate, burst } => {
                if rate == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket rate must be >= 1, got {rate}"
                    )));
                }
                if burst == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket burst must be >= 1, got {burst}"
                    )));
                }
                Ok(())
            },
            Self::Concurrency { max } => {
                if max == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "concurrency max must be >= 1, got {max}"
                    )));
                }
                Ok(())
            },
        }
    }
}

/// A construction-time pacing-config failure.
///
/// The boot-time guard that turns a missing or nonsensical bucket into a
/// startup error instead of a live 429 (ADR-0034 §3). Non-generic: the
/// offending key is rendered to a `String` so `stack()`/`build()` can return
/// `Result<_, BuildError>` regardless of `K`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// A [`RateKey`] variant is not classified in `local` — the map is not
    /// total over `K::all()`.
    #[error(
        "rate-limit key `{0}` is not classified in the config (every RateKey::all() variant must be declared)"
    )]
    UndeclaredKey(String),
    /// A policy carries out-of-range parameters (`rate`/`burst`/`max` of 0).
    #[error("invalid rate-limit policy: {0}")]
    InvalidPolicy(String),
}

/// Validate that `cfg` is a **total**, param-sane pacing configuration.
///
/// The `global` policy is valid, and every [`RateKey`] variant is classified
/// with a valid policy (ADR-0034 §3). Slice 2's `stack()`/`build()` call this
/// before assembling the stack, so a coverage gap is a boot failure.
///
/// # Errors
/// [`BuildError::UndeclaredKey`] if a `K::all()` variant is absent from
/// `cfg.local`; [`BuildError::InvalidPolicy`] if the global or any local policy
/// has an out-of-range parameter.
pub fn validate_coverage<K>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>
where
    K: RateKey + fmt::Debug,
{
    cfg.global.validate()?;
    for key in K::all() {
        match cfg.local.get(key) {
            None => return Err(BuildError::UndeclaredKey(format!("{key:?}"))),
            Some(LimitDecl::Policy(policy)) => policy.validate()?,
            Some(LimitDecl::GlobalOnly) => {},
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_coverage};
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

    /// A total, param-sane config over `TestKey` — the baseline the negative
    /// tests mutate.
    fn total_config() -> RateLimitConfig<TestKey> {
        RateLimitConfig {
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
        }
    }

    #[test]
    fn total_config_validates() {
        assert_eq!(validate_coverage(&total_config()), Ok(()));
    }

    #[test]
    fn missing_key_is_undeclared() {
        let mut cfg = total_config();
        cfg.local.remove(&TestKey::History);
        let err = validate_coverage(&cfg).unwrap_err();
        assert!(matches!(err, BuildError::UndeclaredKey(ref k) if k.contains("History")));
    }

    #[test]
    fn zero_rate_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 0, burst: 5 }),
        );
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn zero_burst_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, burst: 0 }),
        );
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn zero_concurrency_max_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::PlaceOrder,
            LimitDecl::Policy(LimitPolicy::Concurrency { max: 0 }),
        );
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn bad_global_policy_is_invalid() {
        let mut cfg = total_config();
        cfg.global = LimitPolicy::TokenBucket { rate: 0, burst: 1 };
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn global_only_endpoints_need_no_local_params() {
        // A `GlobalOnly` decl carries no policy, so it is always coverage-valid
        // (it is paced by the already-validated global).
        let cfg = RateLimitConfig {
            global: LimitPolicy::Concurrency { max: 2 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::GlobalOnly),
                (TestKey::Snapshot, LimitDecl::GlobalOnly),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(validate_coverage(&cfg), Ok(()));
    }
}
