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
    // `Err`/`Hang` drive the Task 2 full-stack tests (circuit-trip, send-timeout).
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
        // Used by the Task 2 full-stack tests (call-count and Auth re-stamp assertions).
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
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
    // Used by the Task 2 Auth-restamp-per-attempt test.
    #[derive(Clone)]
    struct CounterAuth {
        n: Arc<AtomicUsize>,
    }
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

    // ---- Task 2 tests -----------------------------------------------------

    // 1. CircuitBreaker OUTSIDE Retry — an open circuit fast-rejects; the leaf is
    //    untouched and no retry loop spins on the rejection. If CB were INSIDE
    //    Retry this could not hold: the breaker would be re-consulted per attempt.
    #[tokio::test]
    async fn circuit_opens_and_fast_rejects_without_touching_the_leaf() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Err]); // always fails
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO), // retry ON (3), zero backoff
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        // 3 logical failures (each retried 3x → 9 leaf calls) trip the breaker.
        for _ in 0..3 {
            let _ = svc.call(req(Scope::Global, None)).await;
        }
        let calls_after_trip = leaf.calls();
        assert_eq!(
            calls_after_trip, 9,
            "3 requests x 3 attempts reached the leaf before the trip"
        );
        // Next request: circuit is Open → CircuitOpen, leaf untouched, no spin.
        // `let...else` avoids needing `Debug` on the opaque `Ok` type (see
        // `missing_key_is_a_build_error_and_constructs_nothing` above).
        let Err(err) = svc.call(req(Scope::Global, None)).await else {
            panic!("expected CircuitOpen from an open breaker");
        };
        assert!(matches!(err, HttpError::CircuitOpen));
        assert_eq!(
            leaf.calls(),
            9,
            "open circuit fast-rejects; leaf untouched, Retry never spun"
        );
    }

    // 2. RateLimit INSIDE Retry — each attempt re-acquires budget. With a burst-1
    //    bucket and zero max_wait, the first attempt drains it and the retry
    //    throttles at the (empty) bucket, so the leaf is hit exactly once. If
    //    RateLimit were OUTSIDE Retry, the single token would cover the whole
    //    logical request and the retry would resend to a 200.
    #[tokio::test]
    async fn rate_limit_is_spent_per_attempt_inside_retry() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(503), Step::Status(200)]);
        // Snapshot: burst 1, refill 1/hour → no refill during the test.
        let rc = RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::from([
                (
                    Key::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 1,
                        per: Duration::from_secs(3600),
                        burst: 1,
                    }),
                ),
                (Key::History, LimitDecl::GlobalOnly),
            ]),
        };
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rc,
        )
        .expect("total config");
        // `let...else` avoids needing `Debug` on the opaque `Ok` type.
        let Err(err) = svc.call(req(Scope::Local, Some(Key::Snapshot))).await else {
            panic!("expected Throttled once the per-attempt bucket is drained");
        };
        assert!(
            matches!(err, HttpError::Throttled),
            "the retry re-acquired the drained bucket → per-attempt pacing (RateLimit inside Retry)"
        );
        assert_eq!(
            leaf.calls(),
            1,
            "only attempt 1 reached the leaf; the retry throttled at the bucket"
        );
    }

    // 3. Timeout bounds the SEND. A hanging leaf, with the clock advanced past the
    //    send timeout, yields Timeout. (RateLimit sits outside Timeout, so the
    //    permit wait is bounded separately by rate_limit_max_wait — structural.)
    #[tokio::test]
    async fn send_timeout_fires_on_a_hanging_leaf() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Hang]);
        let svc = stack(
            leaf,
            http_cfg(1, Duration::from_secs(1), Duration::ZERO), // retry OFF, 1s send timeout
            timer.clone(),
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        // `tokio::spawn` needs `F::Output: Send`, but the opaque `HttpClient::Body`
        // carries no `Send` bound (`HttpClient::Body: http_body::Body<..>` only),
        // so a real response body need not be `Send`. `spawn_local` under a
        // `LocalSet` drives the same concurrent interleaving (task registers the
        // sleep, then the timer fires it) without that bound.
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let waiter =
                    tokio::task::spawn_local(
                        async move { svc.call(req(Scope::Global, None)).await },
                    );
                tokio::task::yield_now().await; // task registers the inner sleep + the 1s deadline
                timer.advance(Duration::from_secs(1)); // fire the send-timeout deadline
                // `let...else` avoids needing `Debug` on the opaque `Ok` type.
                let Err(err) = waiter.await.unwrap() else {
                    panic!("expected Timeout once the send deadline fires");
                };
                assert!(matches!(err, HttpError::Timeout));
            })
            .await;
    }

    // 4. Auth re-stamps per attempt — inside Retry, so each of the N attempts
    //    carries a fresh credential.
    #[tokio::test]
    async fn auth_restamps_a_fresh_credential_on_every_attempt() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Err, Step::Err, Step::Status(200)]);
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            CounterAuth::new(),
            rate_cfg(),
        )
        .expect("total config");
        let resp = svc
            .call(req(Scope::Global, None))
            .await
            .expect("third attempt is 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(
            leaf.seen_auth(),
            vec![
                Some("token-0".to_owned()),
                Some("token-1".to_owned()),
                Some("token-2".to_owned())
            ],
            "Auth ran once per attempt (inside Retry), re-stamping a fresh credential each time"
        );
    }

    // 5. Scope fail-closed end-to-end — a request with no RateScope extension is
    //    rejected before the leaf, and the fail-closed path survives composition.
    #[tokio::test]
    async fn absent_scope_is_rejected_fail_closed_through_the_stack() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        // A request with neither RateScope nor Retryable — the forgotten-stamp case.
        let bare = http::Request::builder()
            .method("GET")
            .uri("/x")
            .body(Bytes::new())
            .unwrap();
        // `let...else` avoids needing `Debug` on the opaque `Ok` type.
        let Err(err) = svc.call(bare).await else {
            panic!("expected fail-closed Throttled for a request with no RateScope");
        };
        assert!(
            matches!(err, HttpError::Throttled),
            "no RateScope → fail-closed Throttled"
        );
        assert_eq!(
            leaf.calls(),
            0,
            "the fail-closed request never reached the leaf"
        );
    }
}
