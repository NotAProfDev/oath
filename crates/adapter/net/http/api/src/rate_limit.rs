//! The `RateLimit` resilience layer (ADR-0031 §3).
//!
//! Proactive per-endpoint pacing built from a validated
//! [`RateLimitConfig`], plus the per-request
//! [`RateScope`] directive that selects which buckets a request spends
//! against. Runtime-neutral: generic over
//! [`Timer`], semaphore via `async-lock`.

/// Which bucket sets a request spends against (ADR-0031 §3). Stamped by the
/// adapter as part of a [`RateScope`] request extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    /// Acquire nothing — the **explicit** unlimited opt-out.
    None,
    /// Spend against the account-wide global bucket only.
    Global,
    /// Spend against this endpoint's local bucket only.
    Local,
    /// Spend against both the global and the local bucket.
    Both,
}

/// The per-request pacing directive, carried as an `http::Request` extension.
///
/// The adapter stamps it when it builds each request (it knows the endpoint).
/// An **absent** directive defaults to [`Scope::Global`] — you cannot bypass the
/// account-wide budget by forgetting to stamp. `Clone` so it survives the
/// per-attempt request clone `Retry` performs (Slice 1).
#[derive(Debug, Clone)]
pub struct RateScope<K> {
    /// Which bucket sets to spend against.
    pub scope: Scope,
    /// The endpoint key, required for `Local`/`Both`.
    pub key: Option<K>,
}

use crate::body::Guarded;
use crate::rate::{
    LimitDecl, LimitPolicy, RateLimitConfig, validate_concurrency_singleton, validate_coverage,
};
use crate::{BuildError, HttpError, RateKey, Service};
use async_lock::{Semaphore, SemaphoreGuardArc};
use bytes::Bytes;
use futures_util::future::{Either, select};
use oath_adapter_net_api::{Layer, Timer};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A refilling token-bucket's mutable state (ADR-0031 §3). Guarded by a
/// `std::sync::Mutex` that is **always released before any `await`**.
struct TokenState {
    tokens: f64,
    last: Instant,
}

/// One endpoint's (or the global) pacing state.
enum Bucket {
    /// A token bucket: `refill_per_sec` tokens/second, capped at `burst`.
    Rate {
        refill_per_sec: f64,
        burst: f64,
        state: Mutex<TokenState>,
    },
    /// A concurrency semaphore with `max` permits.
    Concurrency(Arc<Semaphore>),
}

impl Bucket {
    fn build(policy: LimitPolicy, now: Instant) -> Self {
        match policy {
            LimitPolicy::TokenBucket { rate, per, burst } => Self::Rate {
                refill_per_sec: f64::from(rate) / per.as_secs_f64(),
                burst: f64::from(burst),
                state: Mutex::new(TokenState {
                    tokens: f64::from(burst),
                    last: now,
                }),
            },
            LimitPolicy::Concurrency { max } => Self::Concurrency(Arc::new(Semaphore::new(
                usize::try_from(max).unwrap_or(usize::MAX),
            ))),
        }
    }
}

/// The frozen bucket map — key set fixed at construction, so lookup is lock-free
/// and each bucket owns its own lock (contention scoped to one endpoint).
struct RateState<K> {
    global: Bucket,
    local: HashMap<K, Bucket>,
}

/// The `RateLimit` [`Layer`] factory: holds the shared, validated bucket state
/// and produces a [`RateLimit`] around any inner service.
pub struct RateLimitLayer<K, T> {
    state: Arc<RateState<K>>,
    timer: T,
    max_wait: Duration,
}

impl<K, T> Clone for RateLimitLayer<K, T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            timer: self.timer.clone(),
            max_wait: self.max_wait,
        }
    }
}

impl<K, T> fmt::Debug for RateLimitLayer<K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitLayer")
            .field("max_wait", &self.max_wait)
            .finish_non_exhaustive()
    }
}

impl<K, T> RateLimitLayer<K, T> {
    /// Build the pacing layer from a config, validating coverage and the
    /// ≤1-concurrency-permit invariant at construction (a boot failure).
    ///
    /// `max_wait` bounds the whole acquire: an exhausted bucket backpressures up
    /// to this, then the request returns [`HttpError::Throttled`].
    ///
    /// # Errors
    /// Propagates [`validate_coverage`]'s and [`validate_concurrency_singleton`]'s
    /// [`BuildError`].
    pub fn new(cfg: &RateLimitConfig<K>, timer: T, max_wait: Duration) -> Result<Self, BuildError>
    where
        K: RateKey + fmt::Debug,
        T: Timer,
    {
        validate_coverage(cfg)?;
        validate_concurrency_singleton(cfg)?;
        let now = timer.now();
        let global = Bucket::build(cfg.global, now);
        let mut local = HashMap::new();
        for (key, decl) in &cfg.local {
            if let LimitDecl::Policy(policy) = decl {
                local.insert(key.clone(), Bucket::build(*policy, now));
            }
        }
        Ok(Self {
            state: Arc::new(RateState { global, local }),
            timer,
            max_wait,
        })
    }
}

impl<S, K, T> Layer<S> for RateLimitLayer<K, T>
where
    T: Clone,
{
    type Service = RateLimit<S, K, T>;

    fn layer(&self, inner: S) -> RateLimit<S, K, T> {
        RateLimit {
            inner,
            state: Arc::clone(&self.state),
            timer: self.timer.clone(),
            max_wait: self.max_wait,
        }
    }
}

/// The `RateLimit` middleware: paces each request against its buckets, then
/// returns `http::Response<Guarded<B>>` so a concurrency permit rides the body.
pub struct RateLimit<S, K, T> {
    inner: S,
    state: Arc<RateState<K>>,
    timer: T,
    max_wait: Duration,
}

impl<S, K, T> Clone for RateLimit<S, K, T>
where
    S: Clone,
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: Arc::clone(&self.state),
            timer: self.timer.clone(),
            max_wait: self.max_wait,
        }
    }
}

impl<S, K, T> fmt::Debug for RateLimit<S, K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimit")
            .field("max_wait", &self.max_wait)
            .finish_non_exhaustive()
    }
}

impl<S, K, T> RateLimit<S, K, T>
where
    S: Sync,
    K: RateKey,
    T: Timer,
{
    /// Acquire the buckets `directive` calls for, in the order rate-then-
    /// concurrency (global before local), bounded by a single `max_wait`
    /// deadline. Returns the held concurrency permit (if any) for `Guarded`.
    async fn acquire(
        &self,
        directive: &RateScope<K>,
    ) -> Result<Option<SemaphoreGuardArc>, HttpError> {
        if matches!(directive.scope, Scope::None) {
            return Ok(None);
        }
        let want_global = matches!(directive.scope, Scope::Global | Scope::Both);
        let want_local = matches!(directive.scope, Scope::Local | Scope::Both);

        // Collect applicable buckets, rate-type first (ADR-0031 §3 acquire order).
        let mut rate: Vec<&Bucket> = Vec::new();
        let mut conc: Vec<&Bucket> = Vec::new();
        let deadline = self.timer.now() + self.max_wait;

        // global first, then local
        if want_global {
            push_bucket(&self.state.global, &mut rate, &mut conc);
        }
        if want_local {
            // Fail-closed: `Local`/`Both` require a present key + local bucket,
            // else the request cannot be paced and must not be sent unthrottled.
            let key = directive.key.as_ref().ok_or(HttpError::Throttled)?;
            let bucket = self.state.local.get(key).ok_or(HttpError::Throttled)?;
            push_bucket(bucket, &mut rate, &mut conc);
        }

        for bucket in rate {
            acquire_rate(bucket, &self.timer, deadline).await?;
        }
        let mut held = None;
        for bucket in conc {
            held = Some(acquire_conc(bucket, &self.timer, deadline).await?);
        }
        Ok(held)
    }
}

/// Route a bucket into the rate-first / concurrency-second acquire lists.
fn push_bucket<'a>(bucket: &'a Bucket, rate: &mut Vec<&'a Bucket>, conc: &mut Vec<&'a Bucket>) {
    match bucket {
        Bucket::Rate { .. } => rate.push(bucket),
        Bucket::Concurrency(_) => conc.push(bucket),
    }
}

/// Consume one rate token, refilling from elapsed time; wait (lock released
/// first) until one accrues, or return `Throttled` if that would breach the
/// deadline.
async fn acquire_rate<T: Timer>(
    bucket: &Bucket,
    timer: &T,
    deadline: Instant,
) -> Result<(), HttpError> {
    let Bucket::Rate {
        refill_per_sec,
        burst,
        state,
    } = bucket
    else {
        return Ok(()); // not a rate bucket — nothing to do
    };
    loop {
        let wait = {
            let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
            let now = timer.now();
            let elapsed = now.saturating_duration_since(st.last).as_secs_f64();
            st.tokens = (st.tokens + elapsed * refill_per_sec).min(*burst);
            st.last = now;
            if st.tokens >= 1.0 {
                st.tokens -= 1.0;
                return Ok(());
            }
            Duration::from_secs_f64((1.0 - st.tokens) / refill_per_sec)
        }; // lock dropped here — before any await
        if timer.now() + wait > deadline {
            return Err(HttpError::Throttled);
        }
        timer.sleep(wait).await;
    }
}

/// Acquire a concurrency permit, racing the semaphore against the deadline.
async fn acquire_conc<T: Timer>(
    bucket: &Bucket,
    timer: &T,
    deadline: Instant,
) -> Result<SemaphoreGuardArc, HttpError> {
    let Bucket::Concurrency(sem) = bucket else {
        return Err(HttpError::Throttled); // unreachable given push_bucket, but total
    };
    let remaining = deadline.saturating_duration_since(timer.now());
    let acquire = sem.acquire_arc();
    let sleep = timer.sleep(remaining);
    let mut acquire = std::pin::pin!(acquire);
    let mut sleep = std::pin::pin!(sleep);
    match select(acquire.as_mut(), sleep.as_mut()).await {
        Either::Left((guard, _)) => Ok(guard),
        Either::Right(((), _)) => Err(HttpError::Throttled),
    }
}

impl<S, K, T, B> Service<http::Request<Bytes>> for RateLimit<S, K, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    K: RateKey,
    T: Timer,
    B: http_body::Body + Send,
{
    type Response = http::Response<Guarded<B>>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let directive = req
                .extensions()
                .get::<RateScope<K>>()
                .cloned()
                .unwrap_or(RateScope {
                    scope: Scope::Global,
                    key: None,
                });
            let permit = self.acquire(&directive).await?;
            let resp = self.inner.call(req).await?;
            let (parts, body) = resp.into_parts();
            Ok(http::Response::from_parts(
                parts,
                Guarded::new(body, permit),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RateLimitLayer, RateScope, Scope};
    use crate::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Layer;
    use oath_adapter_net_mock::MockTimer;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Snapshot, // rate: 2 per 1s
        History,  // concurrency: 1
    }
    impl crate::RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Snapshot, Self::History]
        }
    }

    // A canned response body (`Data = Bytes`, `Error = HttpError`): one frame,
    // then end. `is_end_stream()` is `false` until polled, so `Guarded` keeps a
    // concurrency permit riding an unread body — the crux of the permit tests.
    // `Debug` so `Result::unwrap_err` can render the `Ok(Response<Guarded<_>>)`.
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
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

    // An inline leaf `Service` returning a fixed `200` body — an inline double
    // instead of `MockClient`, the same no-cycle choice as `body.rs`
    // (net-http-mock depends on THIS crate, so a dev-dep would recompile it and
    // its `Service` would not unify with the crate-under-test's).
    #[derive(Clone)]
    struct Leaf {
        body: &'static [u8],
    }
    impl Leaf {
        fn ok(body: &'static [u8]) -> Self {
            Self { body }
        }
    }
    impl Service<http::Request<Bytes>> for Leaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let data = Some(Bytes::from_static(self.body));
            async move { Ok(http::Response::new(StubBody { data })) }
        }
    }

    #[test]
    fn rate_scope_round_trips_through_request_extensions() {
        let mut req = http::Request::new(Bytes::new());
        req.extensions_mut().insert(RateScope {
            scope: Scope::Both,
            key: Some(Key::History),
        });
        let got = req
            .extensions()
            .get::<RateScope<Key>>()
            .cloned()
            .expect("directive present");
        assert!(matches!(got.scope, Scope::Both));
        assert_eq!(got.key, Some(Key::History));
    }

    // global 10/s rate; Snapshot 2/s rate; History concurrency 1.
    fn config() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 10,
                per: Duration::from_secs(1),
                burst: 10,
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

    fn layer(timer: MockTimer, max_wait: Duration) -> RateLimitLayer<Key, MockTimer> {
        RateLimitLayer::new(&config(), timer, max_wait).expect("valid config")
    }

    fn req(scope: Scope, key: Option<Key>) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        r.extensions_mut().insert(RateScope { scope, key });
        r
    }

    #[tokio::test]
    async fn a_request_within_budget_passes_and_body_is_guarded() {
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        let resp = svc.call(req(Scope::Global, None)).await.expect("passes");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<Guarded<_>> collects transparently
    }

    #[tokio::test]
    async fn local_rate_bucket_throttles_when_drained_and_refills_on_advance() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        // Snapshot burst = 2: two pass, third throttles with zero max_wait.
        svc.call(req(Scope::Local, Some(Key::Snapshot)))
            .await
            .expect("1st");
        svc.call(req(Scope::Local, Some(Key::Snapshot)))
            .await
            .expect("2nd");
        let err = svc
            .call(req(Scope::Local, Some(Key::Snapshot)))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled)); // HttpError has no PartialEq
        // 2 tokens/sec -> one token after 500ms.
        timer.advance(Duration::from_millis(500));
        svc.call(req(Scope::Local, Some(Key::Snapshot)))
            .await
            .expect("refilled");
    }

    #[tokio::test]
    async fn none_scope_acquires_nothing() {
        let timer = MockTimer::new();
        let svc = layer(timer, Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        for _ in 0..100 {
            svc.call(req(Scope::None, None)).await.expect("unlimited");
        }
    }

    #[tokio::test]
    async fn absent_directive_is_global_paced() {
        let timer = MockTimer::new();
        // global burst 10 -> 11th throttles.
        let svc = layer(timer, Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        for _ in 0..10 {
            svc.call(http::Request::new(Bytes::new()))
                .await
                .expect("within global burst");
        }
        let err = svc
            .call(http::Request::new(Bytes::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled)); // HttpError has no PartialEq
    }

    #[tokio::test]
    async fn concurrency_permit_is_held_until_body_drop() {
        // History concurrency max = 1. First call holds the permit via its
        // (unread) body; a second concurrent acquire must wait, then throttle.
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"data"));
        let held = svc
            .call(req(Scope::Local, Some(Key::History)))
            .await
            .expect("1st permit");
        let err = svc
            .call(req(Scope::Local, Some(Key::History)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Throttled),
            "permit still held by first body"
        );
        drop(held); // releasing the body frees the permit
        svc.call(req(Scope::Local, Some(Key::History)))
            .await
            .expect("permit freed");
    }

    #[tokio::test]
    async fn concurrency_waits_within_max_wait_then_succeeds() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(30)).layer(Leaf::ok(b"data"));
        let held = svc
            .call(req(Scope::Local, Some(Key::History)))
            .await
            .expect("1st permit");
        // Second acquire blocks on the semaphore; spawn it, free the permit, and
        // it completes within max_wait.
        let svc2 = svc.clone();
        let waiter =
            tokio::spawn(async move { svc2.call(req(Scope::Local, Some(Key::History))).await });
        tokio::task::yield_now().await;
        drop(held);
        waiter.await.unwrap().expect("acquired after release");
    }
}
