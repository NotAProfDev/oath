//! The `Timeout` resilience layer (ADR-0031 §1).
//!
//! Bounds how long the inner stack may take to **produce a response** — the
//! *send*, not the pacing-permit wait (`RateLimit` sits outside it, so a
//! throttled request never enters `Timeout`). Races `inner.call(req)` against
//! [`Timer::sleep`]; the deadline winning
//! yields [`HttpError::Timeout`] with the inner
//! future dropped, the inner
//! finishing first yields its `Result` verbatim. **Body-transparent:** the
//! response body is returned untouched. The per-request [`RequestTimeout`]
//! extension overrides the layer default; an absent extension uses the default.
//! Runtime-neutral: generic over [`Timer`], race
//! via `futures-util`.

use crate::{HttpError, Service};
use bytes::Bytes;
use futures_util::future::{Either, select};
use oath_adapter_net_api::{Layer, Timer};
use std::fmt;
use std::future::Future;
use std::time::Duration;

/// A per-request timeout override, carried as an `http::Request` extension.
///
/// The adapter stamps it for an endpoint that needs a non-default bound. `Copy`
/// so it survives the per-attempt request clone `Retry` performs (Slice 1). An
/// **absent** extension uses the layer default — a missing override has no
/// fail-open hazard (the global deadline still applies), so it is not rejected
/// (contrast `RateScope`, ADR-0034 Amendment #1).
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout(pub Duration);

/// The `Timeout` [`Layer`] factory: holds the default deadline + clock and
/// produces a [`Timeout`] around any inner service.
pub struct TimeoutLayer<T> {
    default: Duration,
    timer: T,
}

impl<T> TimeoutLayer<T> {
    /// Build the layer with a default deadline and a [`Timer`] clock.
    ///
    /// The default bounds every request lacking a [`RequestTimeout`] extension.
    /// Infallible — every [`Duration`] is a valid deadline (no config to check).
    #[must_use]
    pub const fn new(default: Duration, timer: T) -> Self {
        Self { default, timer }
    }
}

impl<T: Clone> Clone for TimeoutLayer<T> {
    fn clone(&self) -> Self {
        Self {
            default: self.default,
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for TimeoutLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeoutLayer")
            .field("default", &self.default)
            .finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for TimeoutLayer<T> {
    type Service = Timeout<S, T>;

    fn layer(&self, inner: S) -> Timeout<S, T> {
        Timeout {
            inner,
            default: self.default,
            timer: self.timer.clone(),
        }
    }
}

/// The `Timeout` middleware: races the inner call against a deadline.
///
/// Returns [`HttpError::Timeout`] if the deadline
/// wins. Body-transparent — the inner `http::Response<B>` is returned
/// untouched.
pub struct Timeout<S, T> {
    inner: S,
    default: Duration,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for Timeout<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            default: self.default,
            timer: self.timer.clone(),
        }
    }
}

impl<S, T> fmt::Debug for Timeout<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timeout")
            .field("default", &self.default)
            .finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for Timeout<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
{
    type Response = http::Response<B>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let dur = req
                .extensions()
                .get::<RequestTimeout>()
                .map_or(self.default, |t| t.0);
            // `select` polls `call` first, so a ready inner beats a zero deadline;
            // pinning to the stack makes both futures `Unpin` for `select`.
            let call = std::pin::pin!(self.inner.call(req));
            let nap = std::pin::pin!(self.timer.sleep(dur));
            match select(call, nap).await {
                Either::Left((res, _)) => res, // inner finished first -> its Result verbatim
                Either::Right(((), _)) => Err(HttpError::Timeout), // deadline won -> inner dropped
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RequestTimeout, TimeoutLayer};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::{Layer, Timer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    #[test]
    fn request_timeout_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut()
            .insert(RequestTimeout(Duration::from_secs(3)));
        let got = req
            .extensions()
            .get::<RequestTimeout>()
            .copied()
            .expect("override present");
        assert_eq!(got.0, Duration::from_secs(3));
    }

    // A canned one-frame response body (`Data = Bytes`, `Error = HttpError`) —
    // enough to prove `Timeout` returns the body untouched. `Debug` so
    // `Result::unwrap_err` can render an unexpected `Ok`.
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(body: &'static [u8]) -> Self {
            Self {
                data: Some(Bytes::from_static(body)),
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

    // An inline leaf returning `200` immediately — the fast path. Inline (not
    // `MockClient`) to avoid the net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct FastLeaf {
        body: &'static [u8],
    }
    impl Service<http::Request<Bytes>> for FastLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let body = self.body;
            async move { Ok(http::Response::new(StubBody::new(body))) }
        }
    }

    // An inline leaf that sleeps `delay` on the shared clock before returning —
    // lets a test hold the inner future pending while the layer deadline fires,
    // or (with a finite delay) complete after the deadline would have.
    #[derive(Clone)]
    struct DelayLeaf<T> {
        timer: T,
        delay: Duration,
        completed: Arc<AtomicBool>,
    }
    impl<T: Timer> DelayLeaf<T> {
        fn new(timer: T, delay: Duration) -> Self {
            Self {
                timer,
                delay,
                completed: Arc::new(AtomicBool::new(false)),
            }
        }
        fn completed(&self) -> bool {
            self.completed.load(Ordering::Relaxed)
        }
    }
    impl<T: Timer> Service<http::Request<Bytes>> for DelayLeaf<T> {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let timer = self.timer.clone();
            let delay = self.delay;
            let completed = self.completed.clone();
            async move {
                timer.sleep(delay).await;
                completed.store(true, Ordering::Relaxed);
                Ok(http::Response::new(StubBody::new(b"slow")))
            }
        }
    }

    // An inline leaf returning a `Connection` error immediately.
    #[derive(Clone)]
    struct ErrLeaf;
    impl Service<http::Request<Bytes>> for ErrLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async move { Err(HttpError::connection("reset")) }
        }
    }

    fn req(override_to: Option<Duration>) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        if let Some(d) = override_to {
            r.extensions_mut().insert(RequestTimeout(d));
        }
        r
    }

    #[tokio::test]
    async fn fast_inner_passes_and_body_is_transparent() {
        let svc = TimeoutLayer::new(Duration::from_secs(1), MockTimer::new())
            .layer(FastLeaf { body: b"ok" });
        let resp = svc.call(req(None)).await.expect("inner within deadline");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<B> passed straight through
    }

    #[tokio::test]
    async fn slow_inner_times_out_at_default() {
        let timer = MockTimer::new();
        let leaf = DelayLeaf::new(timer.clone(), Duration::from_secs(3600));
        let leaf_probe = leaf.clone();
        let svc = TimeoutLayer::new(Duration::from_secs(1), timer.clone()).layer(leaf);
        let waiter = tokio::spawn(async move { svc.call(req(None)).await });
        tokio::task::yield_now().await; // task registers inner sleep(3600s) + deadline sleep(1s)
        timer.advance(Duration::from_secs(1)); // fire the layer deadline
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Timeout)); // HttpError has no PartialEq
        assert!(
            !leaf_probe.completed(),
            "inner future must be dropped, not run to completion"
        );
    }

    #[tokio::test]
    async fn request_timeout_override_shortens_deadline() {
        // Layer default is huge; a per-request 1s override fires first.
        let timer = MockTimer::new();
        let svc = TimeoutLayer::new(Duration::from_secs(3600), timer.clone())
            .layer(DelayLeaf::new(timer.clone(), Duration::from_secs(3600)));
        let waiter = tokio::spawn(async move { svc.call(req(Some(Duration::from_secs(1)))).await });
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // fires the override, not the default
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Timeout));
    }

    #[tokio::test]
    async fn request_timeout_override_lengthens_deadline() {
        // Default 1s would time out; a 5s override lets the 2s inner complete.
        let timer = MockTimer::new();
        let svc = TimeoutLayer::new(Duration::from_secs(1), timer.clone())
            .layer(DelayLeaf::new(timer.clone(), Duration::from_secs(2)));
        let waiter = tokio::spawn(async move { svc.call(req(Some(Duration::from_secs(5)))).await });
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // now=1s: neither the 2s inner nor the 5s override is due
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // now=2s: the inner completes first
        let resp = waiter
            .await
            .unwrap()
            .expect("override outlived the default; inner completed");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"slow"));
    }

    #[tokio::test]
    async fn inner_error_passes_through_not_masked_as_timeout() {
        let svc = TimeoutLayer::new(Duration::from_secs(1), MockTimer::new()).layer(ErrLeaf);
        let err = svc.call(req(None)).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_))); // its own error, never Timeout
    }

    #[tokio::test]
    async fn zero_default_still_returns_ready_inner() {
        // `select` polls the inner call first, so a ready inner is never
        // preempted by a Duration::ZERO deadline.
        let svc =
            TimeoutLayer::new(Duration::ZERO, MockTimer::new()).layer(FastLeaf { body: b"ok" });
        svc.call(req(None))
            .await
            .expect("inner polled first, not the zero deadline");
    }
}
