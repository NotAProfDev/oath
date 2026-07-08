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
///
/// # Example
/// ```no_run
/// use oath_adapter_net_http_hyper::{build, ConnConfig, TlsTrust, TokioTimer};
/// use oath_adapter_net_http_api::{
///     HttpConfig, NoAuth, RateKey, RateLimitConfig, LimitDecl, LimitPolicy,
///     RetryConfig, CircuitBreakerConfig,
/// };
/// use std::collections::HashMap;
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// enum Endpoint { Rest }
/// impl RateKey for Endpoint { fn all() -> &'static [Self] { &[Endpoint::Rest] } }
///
/// let cfg = HttpConfig {
///     timeout: Duration::from_secs(5),
///     retry: RetryConfig { max_attempts: NonZeroU32::new(3).unwrap(), base: Duration::from_millis(50), cap: Duration::from_secs(1), seed: 1 },
///     circuit_breaker: CircuitBreakerConfig { failure_threshold: NonZeroU32::new(3).unwrap(), cooldown: Duration::from_secs(30), retry_after_fallback: Duration::from_secs(900), retry_after_cap: Duration::from_secs(1800), half_open_probes: NonZeroU32::new(1).unwrap() },
///     headers: http::HeaderMap::new(),
///     rate_limit_max_wait: Duration::from_secs(0),
/// };
/// let rates = RateLimitConfig {
///     global: LimitPolicy::TokenBucket { rate: 1000, per: Duration::from_secs(1), burst: 1000 },
///     local: HashMap::from([(Endpoint::Rest, LimitDecl::GlobalOnly)]),
/// };
/// let conn = ConnConfig {
///     pool_max_idle_per_host: 4,
///     pool_idle_timeout: Duration::from_secs(30),
///     connect_timeout: Duration::from_secs(2),
///     tls_trust: TlsTrust::WebpkiRoots,
///     allow_http: false,
///     http2_keep_alive_interval: None,
///     http2_keep_alive_timeout: Duration::from_secs(10),
///     http2_keep_alive_while_idle: false,
/// };
/// let _client = build(cfg, TokioTimer, NoAuth, rates, conn).expect("valid config");
/// ```
pub fn build<T, A, K>(
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
    conn: ConnConfig,
) -> Result<impl HttpClient<Body: Send> + Clone + Send + Sync + 'static, BuildError>
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
    use crate::leaf::{ConnConfig, TlsTrust};
    use crate::timer::TokioTimer;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use oath_adapter_net_http_api::{
        BuildError, CircuitBreakerConfig, HttpConfig, NoAuth, RateKey, RateScope, RetryConfig,
        Service,
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
            tls_trust: TlsTrust::WebpkiRoots,
            allow_http: true, // the build() smoke test talks to a plain-HTTP echo server
            http2_keep_alive_interval: None,
            http2_keep_alive_timeout: Duration::from_secs(10),
            http2_keep_alive_while_idle: false,
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
                retry_after_fallback: Duration::from_secs(900),
                retry_after_cap: Duration::from_secs(1800),
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

    // A server that fails the FIRST connection (accept then drop → connection reset)
    // and echoes "ok" on every subsequent one — for the retry-over-a-real-reset test.
    async fn spawn_fail_then_ok() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                if first {
                    first = false;
                    drop(stream); // reset the first connection before any response
                    continue;
                }
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

    // A server that always replies with a fixed status code (empty body).
    async fn spawn_status(code: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |_r| async move {
                        let mut resp =
                            hyper::Response::new(http_body_util::Full::new(Bytes::new()));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok::<_, Infallible>(resp)
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    // Build a request with an explicit Global scope + the Retryable opt-in.
    fn req_retryable(url: String) -> http::Request<Bytes> {
        let mut r = http::Request::get(url).body(Bytes::new()).unwrap();
        r.extensions_mut().insert(RateScope::<Key>::Global);
        r.extensions_mut()
            .insert(oath_adapter_net_http_api::Retryable);
        r
    }

    #[tokio::test]
    async fn build_assembles_a_working_stack_over_the_hyper_leaf() {
        let base = spawn_echo().await;
        let client = build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");

        let mut req = http::Request::get(format!("{base}/x"))
            .body(Bytes::new())
            .unwrap();
        // Fail-closed pacing: every request carries an explicit RateScope (ADR-0034 #1).
        req.extensions_mut().insert(RateScope::<Key>::Global);

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
        // return bound (`impl HttpClient<Body: Send> + Clone + Send + Sync + 'static`)
        // deliberately omits; `let...else` extracts the error without needing it.
        let Err(err) = build(http_cfg(), TokioTimer, NoAuth, rates, conn()) else {
            panic!("missing coverage must fail closed");
        };
        assert!(
            matches!(err, BuildError::UndeclaredKey(_)),
            "expected UndeclaredKey, got {err:?}"
        );
    }

    // A real dropped connection on the first attempt maps to HttpError::Connection
    // (H1/H2), which Retry treats as transient — the 2nd attempt reaches the healthy
    // server and returns 200. Observes end-to-end what stack.rs only reasons about.
    #[tokio::test]
    async fn a_real_connection_reset_is_retried_over_the_hyper_leaf() {
        let base = spawn_fail_then_ok().await;
        // retry ON (2 attempts), zero backoff (real clock, no wait).
        let mut cfg = http_cfg();
        cfg.retry.max_attempts = NonZeroU32::new(2).unwrap();
        let client =
            build(cfg, TokioTimer, NoAuth, total_rates(), conn()).expect("total config builds");
        let resp = client
            .call(req_retryable(format!("{base}/x")))
            .await
            .expect("2nd attempt after a real reset succeeds");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok"));
    }

    // A real venue 429 arrives as Ok(status = 429). A single 429 maps to
    // Class::TripNow in the breaker (circuit_breaker.rs classify), which trips
    // immediately on the long retry_after_fallback regardless of failure_threshold; the
    // next call fast-rejects with CircuitOpen without a send.
    #[tokio::test]
    async fn a_real_429_trips_the_breaker_through_the_full_stack() {
        let base = spawn_status(429).await;
        let client = build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");
        // One 429 trips immediately on the long cooldown (throttle path).
        let resp = client
            .call(req_retryable(format!("{base}/x")))
            .await
            .expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        let Err(err) = client.call(req_retryable(format!("{base}/x"))).await else {
            panic!("expected CircuitOpen after a 429 trip");
        };
        assert!(matches!(
            err,
            oath_adapter_net_http_api::HttpError::CircuitOpen
        ));
    }

    // The Timeout layer bounds a real send: a server that never responds (accept +
    // hold) yields HttpError::Timeout at a short real send timeout.
    #[tokio::test]
    async fn send_timeout_fires_over_a_real_hanging_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (_stream, _) = listener.accept().await.unwrap();
                // Hold the connection open, never respond.
                std::future::pending::<()>().await;
            }
        });
        let base = format!("http://{addr}");
        let mut cfg = http_cfg();
        cfg.timeout = Duration::from_millis(200); // short real send bound
        let client =
            build(cfg, TokioTimer, NoAuth, total_rates(), conn()).expect("total config builds");
        let Err(err) = client.call(req_retryable(format!("{base}/x"))).await else {
            panic!("expected Timeout from a hanging server");
        };
        assert!(matches!(err, oath_adapter_net_http_api::HttpError::Timeout));
    }
}
