//! The `RateLimit` resilience layer (ADR-0031 §3).
//!
//! Proactive per-endpoint pacing built from a validated
//! [`RateLimitConfig`], plus the per-request
//! [`RateScope`] directive that selects which buckets a request spends
//! against. Runtime-neutral: generic over
//! [`Timer`], semaphore via `async-lock`.

/// The per-request pacing directive, carried as an `http::Request` extension.
///
/// It names which bucket sets a request spends against (ADR-0031 §3) and carries
/// the endpoint key **inline** on the endpoint-scoped variants, so an illegal
/// "local scope with no key" is unrepresentable (M6).
///
/// The adapter stamps it when it builds each request (it knows the endpoint).
/// An **absent** directive is **rejected fail-closed** (`HttpError::Throttled`,
/// never sent) — a forgotten stamp must not silently fly global-paced-only,
/// skipping the endpoint's own local limit (ADR-0034 Amendment #1). "Global
/// only" is said with an explicit [`RateScope::Global`]; opt out with
/// [`RateScope::None`]. `Clone` so it survives the per-attempt request clone
/// `Retry` performs (Slice 1).
///
/// # Example
/// Stamp the mandatory per-request pacing directive before calling the client
/// (an absent `RateScope` fails closed with [`HttpError::Throttled`]):
/// ```
/// use oath_adapter_net_http_api::RateScope;
///
/// #[derive(Clone, Copy)]
/// enum Endpoint { Orders }
///
/// let mut req = http::Request::new(bytes::Bytes::new());
/// req.extensions_mut().insert(RateScope::Local(Endpoint::Orders));
/// assert!(req.extensions().get::<RateScope<Endpoint>>().is_some());
/// ```
#[derive(Debug, Clone)]
pub enum RateScope<K> {
    /// Acquire nothing — the **explicit** unlimited opt-out.
    None,
    /// Spend against the account-wide global bucket only.
    Global,
    /// Spend against this endpoint's local bucket only.
    Local(K),
    /// Spend against both the global and the local bucket.
    Both(K),
}

use crate::body::{BufferMode, Guarded};
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
        // Concurrency-test note (loom): held only for the brief refill/consume
        // critical section in `acquire_rate` below, and NEVER across an `.await`
        // (the lock is dropped before `timer.sleep`). A loom interleaving model
        // would add little over the clock-injected unit tests. Deferred
        // deliberately (Tier-1 PR8/#101); revisit if the lock scope ever grows to
        // span an await or the contention model changes.
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
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::{RateLimitConfig, RateLimitLayer, LimitDecl, LimitPolicy, RateKey};
    /// use oath_adapter_net_mock::MockTimer;
    /// use std::collections::HashMap;
    /// use std::time::Duration;
    ///
    /// #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    /// enum Endpoint { Orders }
    /// impl RateKey for Endpoint { fn all() -> &'static [Self] { &[Endpoint::Orders] } }
    ///
    /// let cfg = RateLimitConfig {
    ///     global: LimitPolicy::TokenBucket { rate: 10, per: Duration::from_secs(1), burst: 10 },
    ///     local: HashMap::from([(Endpoint::Orders, LimitDecl::GlobalOnly)]),
    /// };
    /// let layer = RateLimitLayer::new(&cfg, MockTimer::new(), Duration::from_secs(0));
    /// assert!(layer.is_ok());
    /// ```
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
        // The key rides the endpoint-scoped variants, so "local scope with no key"
        // is unrepresentable (M6). `None` acquires nothing.
        let (want_global, key) = match directive {
            RateScope::None => return Ok(None),
            RateScope::Global => (true, None),
            RateScope::Local(key) => (false, Some(key)),
            RateScope::Both(key) => (true, Some(key)),
        };

        let deadline = crate::clock::deadline(self.timer.now(), self.max_wait);

        // At most two buckets apply (global and/or local). Collect them into fixed
        // slots — global first — so acquisition needs no per-request allocation (M8).
        let global = want_global.then_some(&self.state.global);
        let local = match key {
            // Fail-closed: a `Local`/`Both` key with no local bucket (e.g. a
            // GlobalOnly endpoint) cannot be paced and must not be sent unthrottled.
            Some(key) => Some(self.state.local.get(key).ok_or(HttpError::Throttled)?),
            None => None,
        };
        let buckets = [global, local];

        // Rate-then-concurrency acquire order (ADR-0031 §3). A rate token spent here
        // is not refunded if a later phase throttles; over-pacing is the safe
        // direction (never a 429).
        for bucket in buckets.into_iter().flatten() {
            if let Bucket::Rate { .. } = bucket {
                acquire_rate(bucket, &self.timer, deadline).await?;
            }
        }
        // `validate_concurrency_singleton` guarantees at most one concurrency bucket
        // per acquire, so the single held permit is unambiguous.
        let mut held = None;
        for bucket in buckets.into_iter().flatten() {
            if let Bucket::Concurrency(_) = bucket {
                held = Some(acquire_conc(bucket, &self.timer, deadline).await?);
            }
        }
        Ok(held)
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
        // unreachable: push_bucket routes rate buckets here; fail closed if ever reached
        return Err(HttpError::Throttled);
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
            // per is validated non-zero, tokens in [0,1), refill_per_sec > 0 -> finite positive wait; no panic.
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
        // unreachable: push_bucket routes rate buckets here; fail closed if ever reached
        return Err(HttpError::Throttled);
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
            let route = crate::meter::route_label(&req);
            // Absent directive fails closed (ADR-0034 Amendment #1): a forgotten
            // stamp must never fly unpaced or global-only, silently skipping the
            // endpoint's own local limit. "Global only" is an explicit
            // RateScope::Global. A fail-closed reject is a local pacing rejection —
            // count it (it is the most likely real-world C1 trigger).
            let Some(directive) = req.extensions().get::<RateScope<K>>().cloned() else {
                crate::meter::throttled(route);
                return Err(HttpError::Throttled);
            };
            // M4 (ADR-0034 §2): a buffered response is fully transferred by the time
            // `call` returns, so release its concurrency permit now instead of
            // letting it ride the in-memory body until the caller drains it.
            let buffered = matches!(
                req.extensions().get::<BufferMode>(),
                Some(BufferMode::Buffer)
            );
            let started = self.timer.now();
            let permit = match self.acquire(&directive).await {
                Ok(permit) => permit,
                // A local pacing rejection — the request was never sent (deep review §2C).
                Err(err) => {
                    crate::meter::throttled(route);
                    return Err(err);
                },
            };
            crate::meter::permit_wait(route, self.timer.now().saturating_duration_since(started));
            let resp = self.inner.call(req).await?;
            let (parts, body) = resp.into_parts();
            let permit = if buffered { None } else { permit };
            Ok(http::Response::from_parts(
                parts,
                Guarded::new(body, permit),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferMode, RateLimitLayer, RateScope};
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
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Leaf {
        fn ok(body: &'static [u8]) -> Self {
            Self {
                body,
                calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        /// How many times this leaf was called — asserts a fail-closed request
        /// never reached it.
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for Leaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let data = Some(Bytes::from_static(self.body));
            async move { Ok(http::Response::new(StubBody { data })) }
        }
    }

    #[test]
    fn rate_scope_round_trips_through_request_extensions() {
        let mut req = http::Request::new(Bytes::new());
        req.extensions_mut().insert(RateScope::Both(Key::History));
        let got = req
            .extensions()
            .get::<RateScope<Key>>()
            .cloned()
            .expect("directive present");
        assert!(matches!(got, RateScope::Both(Key::History)));
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

    fn req(scope: RateScope<Key>) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        r.extensions_mut().insert(scope);
        r
    }

    // Same as `req`, plus a `BufferMode::Buffer` stamp — for the M4 permit-release
    // test (a buffered response frees its concurrency permit at `call`-return).
    fn req_buffered(scope: RateScope<Key>) -> http::Request<Bytes> {
        let mut r = req(scope);
        r.extensions_mut().insert(BufferMode::Buffer);
        r
    }

    #[tokio::test]
    async fn a_request_within_budget_passes_and_body_is_guarded() {
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        let resp = svc.call(req(RateScope::Global)).await.expect("passes");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<Guarded<_>> collects transparently
    }

    #[tokio::test]
    async fn local_rate_bucket_throttles_when_drained_and_refills_on_advance() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        // Snapshot burst = 2: two pass, third throttles with zero max_wait.
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1st");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2nd");
        let err = svc
            .call(req(RateScope::Local(Key::Snapshot)))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled)); // HttpError has no PartialEq
        // 2 tokens/sec -> one token after 500ms.
        timer.advance(Duration::from_millis(500));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("refilled");
    }

    #[tokio::test]
    async fn none_scope_acquires_nothing() {
        let timer = MockTimer::new();
        let svc = layer(timer, Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        for _ in 0..100 {
            svc.call(req(RateScope::None)).await.expect("unlimited");
        }
    }

    #[test]
    fn a_throttled_request_emits_the_throttled_metric() {
        use futures_util::FutureExt;
        use metrics_util::debugging::{DebugValue, DebuggingRecorder};
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
            // Snapshot burst = 2: two pass, the third is a local pacing rejection.
            // MockTimer + max_wait = 0 resolve synchronously, so `now_or_never` drives it.
            svc.call(req(RateScope::Local(Key::Snapshot)))
                .now_or_never()
                .expect("sync")
                .unwrap();
            svc.call(req(RateScope::Local(Key::Snapshot)))
                .now_or_never()
                .expect("sync")
                .unwrap();
            svc.call(req(RateScope::Local(Key::Snapshot)))
                .now_or_never()
                .expect("sync")
                .unwrap_err();
        });
        let throttled = snap.snapshot().into_vec().into_iter().any(|(k, _, _, v)| {
            k.key().name() == "http_rate_limit_throttled_total"
                && matches!(v, DebugValue::Counter(n) if n >= 1)
        });
        assert!(
            throttled,
            "a local pacing rejection emits the throttled counter"
        );
    }

    #[tokio::test]
    async fn absent_directive_fails_closed() {
        // A request with no RateScope extension is rejected, never sent
        // (ADR-0034 Amendment #1) — "global only" must be an explicit RateScope::Global.
        let leaf = Leaf::ok(b"ok");
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(leaf.clone());
        let err = svc
            .call(http::Request::new(Bytes::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled)); // HttpError has no PartialEq
        assert_eq!(leaf.calls(), 0, "absent directive must not reach the leaf");
    }

    #[tokio::test]
    async fn concurrency_permit_is_held_until_body_drop() {
        // History concurrency max = 1. First call holds the permit via its
        // (unread) body; a second concurrent acquire must wait, then throttle.
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"data"));
        let held = svc
            .call(req(RateScope::Local(Key::History)))
            .await
            .expect("1st permit");
        let err = svc
            .call(req(RateScope::Local(Key::History)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Throttled),
            "permit still held by first body"
        );
        drop(held); // releasing the body frees the permit
        svc.call(req(RateScope::Local(Key::History)))
            .await
            .expect("permit freed");
    }

    #[tokio::test]
    async fn buffered_response_releases_concurrency_permit_at_call_return() {
        // M4 (ADR-0034 §2): with `BufferMode::Buffer` the transfer is complete by the
        // time `call` returns, so the concurrency permit must NOT ride the (unread)
        // in-memory body. Contrast `concurrency_permit_is_held_until_body_drop`
        // (streaming), where the unread body keeps the permit.
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(Leaf::ok(b"data"));
        let _first = svc
            .call(req_buffered(RateScope::Local(Key::History)))
            .await
            .expect("1st acquires, then releases the permit at call-return");
        // The permit is already free even though `_first`'s body is never polled.
        svc.call(req_buffered(RateScope::Local(Key::History)))
            .await
            .expect("buffered permit released at call-return, not held by the body");
    }

    #[tokio::test]
    async fn concurrency_waits_within_max_wait_then_succeeds() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(30)).layer(Leaf::ok(b"data"));
        let held = svc
            .call(req(RateScope::Local(Key::History)))
            .await
            .expect("1st permit");
        // Second acquire blocks on the semaphore; spawn it, free the permit, and
        // it completes within max_wait.
        let svc2 = svc.clone();
        let waiter =
            tokio::spawn(async move { svc2.call(req(RateScope::Local(Key::History))).await });
        tokio::task::yield_now().await;
        drop(held);
        waiter.await.unwrap().expect("acquired after release");
    }

    // Snapshot has a local bucket; reclassify it GlobalOnly so it has NONE.
    fn config_with_globalonly() -> RateLimitConfig<Key> {
        let mut cfg = config();
        cfg.local.insert(Key::Snapshot, LimitDecl::GlobalOnly); // Snapshot now has NO local bucket
        cfg
    }

    #[tokio::test]
    async fn local_scope_on_a_globalonly_key_fails_closed() {
        let l = RateLimitLayer::new(
            &config_with_globalonly(),
            MockTimer::new(),
            Duration::from_secs(0),
        )
        .expect("valid config");
        let leaf = Leaf::ok(b"ok");
        let svc = l.layer(leaf.clone());
        let err = svc
            .call(req(RateScope::Local(Key::Snapshot)))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled));
        assert_eq!(leaf.calls(), 0, "must never reach the leaf");
    }

    #[tokio::test]
    async fn sub_one_per_second_rate_admits_one_then_throttles_until_window() {
        // 1 token per 5s, burst 1: one request passes, the next throttles until 5s elapse.
        let mut cfg = config();
        cfg.local.insert(
            Key::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket {
                rate: 1,
                per: Duration::from_secs(5),
                burst: 1,
            }),
        );
        let timer = MockTimer::new();
        let svc = RateLimitLayer::new(&cfg, timer.clone(), Duration::from_secs(0))
            .expect("valid config")
            .layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1st admitted");
        let err = svc
            .call(req(RateScope::Local(Key::Snapshot)))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Throttled));
        timer.advance(Duration::from_secs(5));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("refilled after 5s");
    }

    #[tokio::test]
    async fn rate_park_loop_sleeps_then_refills_and_succeeds() {
        // Snapshot = 2/s burst 2. Drain both tokens, then a third request with a
        // GENEROUS max_wait must PARK in acquire_rate (timer.sleep), not throttle.
        // Advancing the clock past the refill window wakes it and it succeeds — the
        // proactive wait+refill path that every max_wait=0 test skips.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(5)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1st drains a token");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2nd drains the last token");

        // Third: bucket empty, but max_wait = 5s > the 500ms refill interval → it must
        // park on timer.sleep rather than return Throttled. Spawn it, let it register
        // the sleep, then advance the clock to refill one token and wake it.
        let svc2 = svc.clone();
        let waiter =
            tokio::spawn(async move { svc2.call(req(RateScope::Local(Key::Snapshot))).await });
        tokio::task::yield_now().await; // task locks the bucket, sees empty, arms timer.sleep
        timer.advance(Duration::from_millis(500)); // 2 tokens/sec → +1 token, wakes the sleeper
        waiter
            .await
            .unwrap()
            .expect("parked request refilled within max_wait and succeeded");
    }

    #[tokio::test]
    async fn refill_rate_is_exact_not_just_a_lower_bound() {
        // Snapshot = 2 tokens/sec, burst 2. Drain both, then advance ONLY 500ms so the
        // correct refill is exactly 1 token (0.5s × 2/s = 1) — strictly BELOW burst, so
        // the burst cap can't mask an inflated rate. Admit exactly 1; the next throttles.
        // A 2x-over-refill bug credits 2 tokens in 500ms → admits a 2nd → this fails,
        // WITHOUT needing the burst cap to also be broken.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2");
        assert!(matches!(
            svc.call(req(RateScope::Local(Key::Snapshot)))
                .await
                .unwrap_err(),
            HttpError::Throttled
        ));
        timer.advance(Duration::from_millis(500)); // exactly 1 token, < burst 2
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("exactly 1 token refilled");
        assert!(
            matches!(
                svc.call(req(RateScope::Local(Key::Snapshot)))
                    .await
                    .unwrap_err(),
                HttpError::Throttled
            ),
            "only 1 token accrued in 500ms (rate=2/s) — a 2nd admit would mean an over-refill"
        );
    }

    #[tokio::test]
    async fn partial_period_does_not_over_refill() {
        // 2 tokens/sec: after only 250ms (< the 500ms/token interval) NO token has
        // accrued, so a drained bucket still throttles. Catches an off-by-one-fast
        // refill that credits a fractional token as whole.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2");
        timer.advance(Duration::from_millis(250)); // < 500ms → no whole token yet
        assert!(
            matches!(
                svc.call(req(RateScope::Local(Key::Snapshot)))
                    .await
                    .unwrap_err(),
                HttpError::Throttled
            ),
            "a quarter-period must not refill a whole token"
        );
    }

    #[tokio::test]
    async fn both_scope_spends_global_and_local_in_one_acquire() {
        // Both(Snapshot) must acquire the global bucket AND the Snapshot local bucket.
        // Snapshot burst = 2 is the tighter of the two (global burst = 10), so the 3rd
        // Both request throttles on the drained LOCAL bucket — proving both buckets are
        // consulted (a Both that only spent global would admit up to 10).
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Both(Key::Snapshot)))
            .await
            .expect("1 (global+local)");
        svc.call(req(RateScope::Both(Key::Snapshot)))
            .await
            .expect("2 (global+local)");
        assert!(
            matches!(
                svc.call(req(RateScope::Both(Key::Snapshot)))
                    .await
                    .unwrap_err(),
                HttpError::Throttled
            ),
            "3rd Both throttles on the drained LOCAL bucket → both buckets were spent"
        );
    }

    #[tokio::test]
    async fn both_scope_throttles_when_only_the_global_bucket_is_empty() {
        // Symmetric: drain the GLOBAL bucket (10/s) via Global-scoped calls, then a
        // Both(History) request — whose local side (concurrency) is free — must still
        // throttle, proving the global side is acquired first and gates a Both request.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        for _ in 0..10 {
            svc.call(req(RateScope::Global))
                .await
                .expect("drain global burst 10");
        }
        assert!(
            matches!(
                svc.call(req(RateScope::Both(Key::History)))
                    .await
                    .unwrap_err(),
                HttpError::Throttled
            ),
            "Both must acquire the (empty) global bucket before its local side"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_burst_admits_at_most_the_burst_size() {
        // Snapshot burst = 2. Fire 8 requests concurrently against a fresh bucket with
        // max_wait = 0. The bucket must admit EXACTLY 2 and throttle the other 6 — no
        // momentary over-admission from a racing consume/refill.
        let timer = MockTimer::new();
        let svc = layer(timer, Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = svc.clone();
            handles.push(tokio::spawn(async move {
                s.call(req(RateScope::Local(Key::Snapshot))).await.is_ok()
            }));
        }
        let mut admitted = 0usize;
        for h in handles {
            if h.await.unwrap() {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, 2,
            "a burst-2 bucket admits exactly its burst under a concurrent burst, no over-admission"
        );
    }
}
