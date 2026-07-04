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
//! first-live-order 429 → 15-minute IBKR penalty box.
//!
//! This module is pure data + its two validators (`validate_coverage`,
//! `validate_concurrency_singleton`); the `RateLimit` layer that consumes them
//! lives in [`crate::rate_limit`].

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::time::Duration;

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
    /// A refilling token bucket: `rate` tokens per `per` window, up to `burst`
    /// in hand. `per` lets sub-1/second venue limits (IBKR `1/5s`, `1/min`,
    /// `1/15min`) be expressed exactly with integer parameters.
    TokenBucket {
        /// Tokens replenished per `per` window (must be `>= 1`).
        rate: u32,
        /// The replenishment window (must be non-zero).
        per: Duration,
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
            Self::TokenBucket { rate, per, burst } => {
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
                if per.is_zero() {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket period must be non-zero, got {per:?}"
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
    /// A config in which a `Both`-scoped request could require two held
    /// concurrency permits (global `Concurrency` **and** a local `Concurrency`)
    /// — [`Guarded`](crate::Guarded) holds one, so this is a boot failure, not a
    /// silent runtime permit truncation.
    #[error(
        "config has both a global and a local Concurrency policy; a Both-scoped request would need two held permits (Guarded holds one)"
    )]
    MultipleConcurrency,
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

/// Reject a config whose `Both`-scoped requests could require two held
/// concurrency permits — global `Concurrency` **and** any local `Concurrency`.
///
/// `RateLimitLayer::new` calls this alongside [`validate_coverage`], turning the
/// ≤1-concurrency-permit invariant into a boot failure (spec Decision 6).
///
/// # Errors
/// [`BuildError::MultipleConcurrency`] if the global policy is `Concurrency` and
/// any local `Policy` is also `Concurrency`.
pub fn validate_concurrency_singleton<K>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>
where
    K: RateKey,
{
    if matches!(cfg.global, LimitPolicy::Concurrency { .. }) {
        for decl in cfg.local.values() {
            if matches!(decl, LimitDecl::Policy(LimitPolicy::Concurrency { .. })) {
                return Err(BuildError::MultipleConcurrency);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig,
        validate_concurrency_singleton, validate_coverage,
    };
    use std::collections::HashMap;
    use std::time::Duration;

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
        // fails to compile HERE, forcing whoever adds it to also update
        // `all()` by hand — that compile error is the actual drift guard.
        // The length assertion only catches `all()` shrinking (e.g. an
        // accidental removal), not a variant omitted from it.
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
                per: Duration::from_secs(1),
                burst: 20,
            },
            local: HashMap::from([
                (
                    TestKey::PlaceOrder,
                    LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 }),
                ),
                (
                    TestKey::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 5,
                        per: Duration::from_secs(1),
                        burst: 5,
                    }),
                ),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(cfg.local.len(), 3);
        assert_eq!(
            cfg.global,
            LimitPolicy::TokenBucket {
                rate: 10,
                per: Duration::from_secs(1),
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
                per: Duration::from_secs(1),
                burst: 20,
            },
            local: HashMap::from([
                (
                    TestKey::PlaceOrder,
                    LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 }),
                ),
                (
                    TestKey::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 5,
                        per: Duration::from_secs(1),
                        burst: 5,
                    }),
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
            LimitDecl::Policy(LimitPolicy::TokenBucket {
                rate: 0,
                per: Duration::from_secs(1),
                burst: 5,
            }),
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
            LimitDecl::Policy(LimitPolicy::TokenBucket {
                rate: 5,
                per: Duration::from_secs(1),
                burst: 0,
            }),
        );
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn zero_period_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket {
                rate: 5,
                per: Duration::ZERO,
                burst: 5,
            }),
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
        cfg.global = LimitPolicy::TokenBucket {
            rate: 0,
            per: Duration::from_secs(1),
            burst: 1,
        };
        assert!(matches!(
            validate_coverage(&cfg),
            Err(BuildError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn token_bucket_carries_a_period_for_sub_1_per_second_rates() {
        // IBKR orders = 1 per 5s — inexpressible as tokens/second under u32.
        let p = LimitPolicy::TokenBucket {
            rate: 1,
            per: Duration::from_secs(5),
            burst: 1,
        };
        assert!(matches!(
            p,
            LimitPolicy::TokenBucket {
                rate: 1,
                burst: 1,
                ..
            }
        ));
    }

    #[test]
    fn global_and_local_concurrency_is_rejected() {
        // Both-scoped request would need two held permits; Guarded holds one.
        let cfg = RateLimitConfig {
            global: LimitPolicy::Concurrency { max: 5 },
            local: HashMap::from([
                (
                    TestKey::PlaceOrder,
                    LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 }),
                ),
                (TestKey::Snapshot, LimitDecl::GlobalOnly),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(
            validate_concurrency_singleton(&cfg),
            Err(BuildError::MultipleConcurrency)
        );
    }

    #[test]
    fn global_rate_with_local_concurrency_is_allowed() {
        // The real IBKR shape: global 10/s rate + /history concurrency.
        assert_eq!(validate_concurrency_singleton(&total_config()), Ok(()));
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
