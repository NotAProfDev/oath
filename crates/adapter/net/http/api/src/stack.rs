//! The `stack()` assembly (ADR-0031 §1) + the non-generic `HttpConfig`.
//!
//! [`stack`] composes the canonical resilience order over any leaf:
//! `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders( Auth( leaf ) ) ) ) ) ) )`.
//! It builds the one fallible layer ([`RateLimitLayer`],
//! which validates pacing coverage + the concurrency-singleton invariant) first,
//! so a coverage/param error is a [`BuildError`] before the
//! rest is assembled. `Auth`/`SetHeaders` are direct `Service` wrappers (no
//! `Layer` factory), so they pre-wrap the leaf; the five `Layer`-factory layers
//! compose over that via [`ServiceBuilder`]
//! (first `.layer()` = outermost). The composed value satisfies
//! [`HttpClient`] by blanket impl.

use crate::rate::{BuildError, RateKey, RateLimitConfig};
use crate::{
    Auth, AuthSource, CircuitBreakerConfig, CircuitBreakerLayer, HttpClient, RateLimitLayer,
    RetryConfig, RetryLayer, SetHeaders, TimeoutLayer, TracingLayer,
};
use oath_adapter_net_api::{ServiceBuilder, Timer};
use std::fmt;
use std::time::Duration;

/// Non-generic assembly configuration for [`stack`].
///
/// One field per configurable layer plus the static default headers. The
/// `K`-generic pacing map (`RateLimitConfig<K>`), `auth`, and `timer` are
/// separate [`stack`] arguments, so this type carries no type parameter and
/// no `serde` (deserialisation stays in the adapter, ADR-0003).
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Per-attempt send timeout — bounds the send, **not** the permit wait.
    pub timeout: Duration,
    /// Retry policy (attempts, backoff schedule).
    pub retry: RetryConfig,
    /// Circuit-breaker thresholds and cooldowns.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Static default request headers, stamped by `SetHeaders` (just outside `Auth`).
    pub headers: http::HeaderMap,
    /// Ceiling on how long an exhausted bucket back-pressures before the request
    /// returns [`HttpError::Throttled`](crate::HttpError::Throttled). Distinct from
    /// `timeout`: `RateLimit` sits **outside** `Timeout`, so the permit wait is
    /// bounded by this — at IBKR's 1/15-min buckets, minutes not seconds.
    pub rate_limit_max_wait: Duration,
}

/// Assemble the canonical resilience stack (ADR-0031 §1) over an arbitrary leaf.
///
/// Builds the fallible [`RateLimit`](crate::RateLimit) layer **first** — it runs
/// `validate_coverage` + `validate_concurrency_singleton` — so a config that is not
/// total over `K::all()`, carries an out-of-range policy param, or breaches the
/// ≤1-concurrency-permit invariant is a [`BuildError`] before the infallible layers
/// are assembled. Then composes, outermost-first:
/// `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders( Auth( leaf ) ) ) ) ) ) )`.
/// `Auth`/`SetHeaders` are direct `Service` wrappers (no `Layer` factory), so they
/// pre-wrap the leaf; the composed value satisfies [`HttpClient`] by blanket impl.
///
/// # Errors
/// [`BuildError`] propagated from `RateLimitLayer::new` if `rate_limits` is not
/// total over `K::all()`, any policy is out of range, or the concurrency-singleton
/// invariant is breached.
// `rate_limits` is only borrowed internally (`RateLimitLayer::new` takes `&RateLimitConfig<K>`),
// but the public signature takes it by value to match `cfg`'s "config consumed once at boot"
// shape — the caller hands the whole aggregate over and is done with it.
#[allow(clippy::needless_pass_by_value)]
pub fn stack<S, T, A, K>(
    leaf: S,
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where
    S: HttpClient + Clone + Send + Sync + 'static,
    S::Body: Send,
    T: Timer + 'static,
    A: AuthSource + 'static,
    K: RateKey + fmt::Debug,
{
    // Fallible layer first: validates coverage + concurrency-singleton (fail-closed
    // at construction — nothing else is built if this errors).
    let rate = RateLimitLayer::new(&rate_limits, timer.clone(), cfg.rate_limit_max_wait)?;
    // The two innermost layers are direct wrappers, not `Layer` factories.
    let inner = SetHeaders::new(Auth::new(leaf, auth), cfg.headers);
    let svc = ServiceBuilder::new()
        .layer(TracingLayer::new(timer.clone())) // outermost
        .layer(CircuitBreakerLayer::new(cfg.circuit_breaker, timer.clone()))
        .layer(RetryLayer::new(cfg.retry, timer.clone()))
        .layer(rate)
        .layer(TimeoutLayer::new(cfg.timeout, timer)) // innermost Layer-factory
        .service(inner);
    Ok(svc)
}

#[cfg(test)]
mod tests {
    use super::{HttpConfig, stack};
    use crate::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use crate::{
        AuthSource, BuildError, CircuitBreakerConfig, HttpError, NoAuth, RateKey, RateScope,
        RetryConfig, Retryable, Scope, Service,
    };
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Timer;
    use oath_adapter_net_mock::MockTimer;
    use std::collections::HashMap;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    // ---- test RateKey ----------------------------------------------------
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Snapshot,
        History,
    }
    impl RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Snapshot, Self::History]
        }
    }

    // ---- canned one-frame body (Data = Bytes, Error = HttpError) ----------
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(b: &'static [u8]) -> Self {
            Self {
                data: Some(Bytes::from_static(b)),
            }
        }
    }
    impl Body for StubBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.data.is_none()
        }
        fn size_hint(&self) -> SizeHint {
            self.data.as_ref().map_or_else(
                || SizeHint::with_exact(0),
                |d| SizeHint::with_exact(d.len() as u64),
            )
        }
    }

    // ---- scripted, recording, clock-aware inline leaf --------------------
    // One scripted outcome per attempt; repeats the last once exhausted. Records
    // the `Authorization` header each call saw (for the Auth re-stamp test) and
    // counts calls (for the untouched-leaf assertions). Inline, not `MockClient`,
    // to avoid the net-http-mock -> net-http-api dev-dep cycle.
    // `Err`/`Hang` are consumed by the Task 2 full-stack tests (circuit-trip,
    // send-timeout) landing later in this module; `#[allow(dead_code)]` is
    // temporary scaffolding until then.
    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    enum Step {
        Status(u16),
        Err,  // connection error (retryable)
        Hang, // sleeps 1h on the shared clock (for the Timeout test)
    }
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
        seen_auth: Arc<Mutex<Vec<Option<String>>>>,
        timer: MockTimer,
    }
    impl ScriptLeaf {
        fn new(timer: MockTimer, steps: Vec<Step>) -> Self {
            Self {
                steps: Arc::new(steps),
                calls: Arc::new(AtomicUsize::new(0)),
                seen_auth: Arc::new(Mutex::new(Vec::new())),
                timer,
            }
        }
        // Consumed by the Task 2 full-stack tests (call-count and Auth re-stamp
        // assertions); `#[allow(dead_code)]` is temporary until then.
        #[allow(dead_code)]
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
        #[allow(dead_code)]
        fn seen_auth(&self) -> Vec<Option<String>> {
            self.seen_auth.lock().unwrap().clone()
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self
                .steps
                .get(i)
                .copied()
                .unwrap_or_else(|| *self.steps.last().unwrap());
            let seen = req
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            self.seen_auth.lock().unwrap().push(seen);
            let timer = self.timer.clone();
            async move {
                match step {
                    Step::Status(code) => {
                        let mut resp = http::Response::new(StubBody::new(b"ok"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    },
                    Step::Err => Err(HttpError::connection("reset")),
                    Step::Hang => {
                        timer.sleep(Duration::from_secs(3600)).await;
                        Ok(http::Response::new(StubBody::new(b"late")))
                    },
                }
            }
        }
    }

    // ---- an AuthSource stamping a monotonically-increasing credential -----
    // Consumed by the Task 2 Auth-restamp-per-attempt test; `#[allow(dead_code)]`
    // is temporary until then.
    #[allow(dead_code)]
    #[derive(Clone)]
    struct CounterAuth {
        n: Arc<AtomicUsize>,
    }
    #[allow(dead_code)]
    impl CounterAuth {
        fn new() -> Self {
            Self {
                n: Arc::new(AtomicUsize::new(0)),
            }
        }
    }
    impl AuthSource for CounterAuth {
        fn authorize(
            &self,
            req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let n = self.n.fetch_add(1, Ordering::Relaxed);
            let val = http::HeaderValue::from_str(&format!("token-{n}")).unwrap();
            req.headers_mut().insert(http::header::AUTHORIZATION, val);
            std::future::ready(Ok(()))
        }
    }

    // ---- config builders --------------------------------------------------
    // Retry/circuit-breaker knobs tuned so pacing never accidentally interferes:
    // a generous global bucket, zero backoff (retries run inline under MockTimer).
    fn http_cfg(retry_attempts: u32, timeout: Duration, max_wait: Duration) -> HttpConfig {
        HttpConfig {
            timeout,
            retry: RetryConfig {
                max_attempts: NonZeroU32::new(retry_attempts).unwrap(),
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
            rate_limit_max_wait: max_wait,
        }
    }
    // Global effectively unlimited; Snapshot 2/s; History concurrency 1.
    fn rate_cfg() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::from([
                (
                    Key::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 2,
                        per: Duration::from_secs(1),
                        burst: 2,
                    }),
                ),
                (
                    Key::History,
                    LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 }),
                ),
            ]),
        }
    }
    fn req(scope: Scope, key: Option<Key>) -> http::Request<Bytes> {
        let mut r = http::Request::builder()
            .method("GET")
            .uri("/x")
            .body(Bytes::new())
            .unwrap();
        r.extensions_mut().insert(RateScope { scope, key });
        r.extensions_mut().insert(Retryable); // opt in so Retry engages when max_attempts > 1
        r
    }

    // ---- Task 1 tests -----------------------------------------------------

    #[tokio::test]
    async fn plain_request_threads_all_layers_and_body_is_transparent() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(
            leaf,
            http_cfg(1, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        let resp = svc.call(req(Scope::Global, None)).await.expect("200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // through all 7 layers + Guarded, untouched
    }

    #[test]
    fn missing_key_is_a_build_error_and_constructs_nothing() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let mut rc = rate_cfg();
        rc.local.remove(&Key::History); // no longer total over Key::all()
        // `.unwrap_err()` would require `Debug` on the opaque `Ok` type, which the
        // return bound (`impl HttpClient + Clone + Send + Sync + 'static`) deliberately
        // omits; `let...else` extracts the error without needing it.
        let Err(err) = stack(
            leaf,
            http_cfg(1, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rc,
        ) else {
            panic!("expected a BuildError for a non-total rate config");
        };
        assert!(matches!(err, BuildError::UndeclaredKey(ref k) if k.contains("History")));
    }
}
