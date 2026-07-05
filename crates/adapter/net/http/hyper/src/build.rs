//! [`build`] — the hyper construction surface.
//!
//! Assembles the canonical resilience stack (ADR-0031 §1) over a fresh pooled
//! hyper leaf by delegating to `oath_adapter_net_http_api::stack`; ordering
//! invariants stay tested there (#88).

use crate::leaf::{ConnConfig, hyper_leaf};
use oath_adapter_net_api::Timer;
use oath_adapter_net_http_api::rate::{BuildError, RateKey, RateLimitConfig};
use oath_adapter_net_http_api::{AuthSource, HttpClient, HttpConfig, stack};
use std::fmt;

/// Assemble the canonical resilience stack over a fresh pooled hyper leaf.
///
/// `build(cfg, timer, auth, rate_limits, conn) == stack(hyper_leaf(conn), cfg,
/// timer, auth, rate_limits)`. The return is fully opaque — adapters use it
/// through the `HttpClient` seam, never naming the concrete layered type.
///
/// # Errors
/// [`BuildError`] from `stack()` if `rate_limits` is not total over `K::all()`,
/// a policy is out of range, or the concurrency-singleton invariant is breached.
pub fn build<T, A, K>(
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
    conn: ConnConfig,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where
    T: Timer + 'static,
    A: AuthSource + 'static,
    K: RateKey + fmt::Debug,
{
    stack(hyper_leaf(conn), cfg, timer, auth, rate_limits)
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::leaf::ConnConfig;
    use crate::timer::TokioTimer;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use oath_adapter_net_http_api::{
        BuildError, CircuitBreakerConfig, HttpConfig, NoAuth, RateKey, RateScope, RetryConfig,
        Scope, Service,
    };
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::num::NonZeroU32;
    use std::time::Duration;
    use tokio::net::TcpListener;

    // A single-variant, drift-proof RateKey (compiler-checked `all()`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Rest,
    }
    impl RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Rest]
        }
    }

    fn conn() -> ConnConfig {
        ConnConfig {
            pool_max_idle_per_host: 4,
            pool_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(2),
        }
    }

    fn http_cfg() -> HttpConfig {
        HttpConfig {
            timeout: Duration::from_secs(5),
            retry: RetryConfig {
                max_attempts: NonZeroU32::new(1).unwrap(),
                base: Duration::ZERO,
                cap: Duration::ZERO,
                seed: 1,
            },
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: NonZeroU32::new(3).unwrap(),
                cooldown: Duration::from_secs(30),
                throttle_cooldown: Duration::from_secs(900),
                half_open_probes: NonZeroU32::new(1).unwrap(),
            },
            headers: http::HeaderMap::new(),
            rate_limit_max_wait: Duration::ZERO,
        }
    }

    // A total pacing config over Key (global unlimited; Rest global-only).
    fn total_rates() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::from([(Key::Rest, LimitDecl::GlobalOnly)]),
        }
    }

    async fn spawn_echo() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(|_r| async {
                        Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                            Bytes::from_static(b"ok"),
                        )))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn build_assembles_a_working_stack_over_the_hyper_leaf() {
        let base = spawn_echo().await;
        let client = build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");

        let mut req = http::Request::get(format!("{base}/x"))
            .body(Bytes::new())
            .unwrap();
        // Fail-closed pacing: every request carries an explicit Scope (ADR-0034 #1).
        req.extensions_mut().insert(RateScope::<Key> {
            scope: Scope::Global,
            key: None,
        });

        let resp = client
            .call(req)
            .await
            .expect("round-trip through the stack");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok"));
    }

    #[test]
    fn build_rejects_a_config_missing_pacing_coverage() {
        // `local` empty ⇒ Key::Rest unclassified ⇒ BuildError before any leaf work.
        let rates = RateLimitConfig::<Key> {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::new(),
        };
        // `.expect_err()` would require `Debug` on the opaque `Ok` type, which the
        // return bound (`impl HttpClient + Clone + Send + Sync + 'static`) deliberately
        // omits; `let...else` extracts the error without needing it.
        let Err(err) = build(http_cfg(), TokioTimer, NoAuth, rates, conn()) else {
            panic!("missing coverage must fail closed");
        };
        assert!(
            matches!(err, BuildError::UndeclaredKey(_)),
            "expected UndeclaredKey, got {err:?}"
        );
    }
}
