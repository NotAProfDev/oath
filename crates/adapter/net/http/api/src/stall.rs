//! The `StallTimeout` body-inactivity layer (ADR-0034 Amendment #13).
//!
//! Wraps a *streaming* response body with a per-frame **inactivity** deadline via
//! the runtime-neutral [`Timer`] seam: if no frame arrives within the configured
//! duration the body yields [`HttpError::Timeout`], so a stalled transfer can no
//! longer wedge a `Guarded` concurrency permit. Placed innermost in `stack()` so
//! `Guarded` wraps [`TimeoutBody`] and the stall error releases the permit.
//! Inert on buffered bodies (one ready frame) and when the deadline is `None`.
//! This guard bounds mid-transfer *inactivity*, not total transferred size: a
//! steady, never-idle `BufferMode::Stream` response is bounded by neither this
//! guard nor the buffered-size cap, by design — in Stream mode the caller owns
//! accumulation, and the cap applies only to `BufferMode::Buffer`.

use crate::{HttpError, Service};
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use oath_adapter_net_api::{Layer, Timer};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

pin_project_lite::pin_project! {
    /// A response body wrapper enforcing a per-frame **inactivity** timeout.
    ///
    /// On each poll it (re-)arms a [`Timer::sleep`] deadline; if the deadline
    /// fires before the inner body produces a frame, it yields
    /// [`HttpError::Timeout`]. Each frame resets the deadline (lazily: `sleep` is
    /// cleared and re-armed on the next poll). A `None` timeout is fully inert —
    /// the wrapper forwards to `inner` unchanged. Forwards `is_end_stream` and
    /// `size_hint` (ADR-0034 §2 transparency).
    pub struct TimeoutBody<B, T: Timer> {
        #[pin]
        inner: B,
        #[pin]
        sleep: Option<T::Sleep>,
        timeout: Option<Duration>,
        timer: T,
    }
}

impl<B, T: Timer> TimeoutBody<B, T> {
    /// Wrap `inner` with a stall timeout. `None` disables it (pass-through).
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::TimeoutBody;
    /// use oath_adapter_net_mock::MockTimer;
    /// use http_body_util::Empty;
    /// use bytes::Bytes;
    /// use std::time::Duration;
    ///
    /// let _body = TimeoutBody::new(
    ///     Empty::<Bytes>::new(),
    ///     Some(Duration::from_secs(30)),
    ///     MockTimer::new(),
    /// );
    /// ```
    #[must_use]
    pub const fn new(inner: B, timeout: Option<Duration>, timer: T) -> Self {
        Self {
            inner,
            sleep: None,
            timeout,
            timer,
        }
    }
}

impl<B: fmt::Debug, T: Timer> fmt::Debug for TimeoutBody<B, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeoutBody")
            .field("inner", &self.inner)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<B, T> Body for TimeoutBody<B, T>
where
    B: Body<Data = Bytes, Error = HttpError>,
    T: Timer,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let mut this = self.project();
        let Some(dur) = *this.timeout else {
            // Disabled: fully transparent.
            return this.inner.poll_frame(cx);
        };
        // Arm the deadline if unset.
        if this.sleep.is_none() {
            let nap = this.timer.sleep(dur);
            this.sleep.set(Some(nap));
        }
        // Poll the timer FIRST so its waker stays registered while the body pends.
        if let Some(sleep) = this.sleep.as_mut().as_pin_mut()
            && sleep.poll(cx).is_ready()
        {
            return Poll::Ready(Some(Err(HttpError::Timeout)));
        }
        let frame = ready!(this.inner.poll_frame(cx));
        this.sleep.set(None); // lazy per-frame reset (inactivity semantics)
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// The [`StallTimeout`] [`Layer`] factory: holds the (optional) inactivity
/// deadline + clock and wraps any inner service's response body in
/// [`TimeoutBody`].
pub struct StallTimeoutLayer<T> {
    timeout: Option<Duration>,
    timer: T,
}

impl<T> StallTimeoutLayer<T> {
    /// Build the layer. `None` disables the stall timeout (pass-through body).
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::StallTimeoutLayer;
    /// use oath_adapter_net_mock::MockTimer;
    /// use std::time::Duration;
    ///
    /// let _layer = StallTimeoutLayer::new(Some(Duration::from_secs(30)), MockTimer::new());
    /// ```
    #[must_use]
    pub const fn new(timeout: Option<Duration>, timer: T) -> Self {
        Self { timeout, timer }
    }
}

impl<T: Clone> Clone for StallTimeoutLayer<T> {
    fn clone(&self) -> Self {
        Self {
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for StallTimeoutLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StallTimeoutLayer")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for StallTimeoutLayer<T> {
    type Wrapped = StallTimeout<S, T>;
    fn layer(&self, inner: S) -> StallTimeout<S, T> {
        StallTimeout {
            inner,
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

/// The `StallTimeout` middleware: wraps the inner service's response body.
///
/// Wraps the response body in a [`TimeoutBody`]. The send itself is untouched
/// (that is the `Timeout` layer's job); only the streamed body gains the
/// inactivity deadline.
pub struct StallTimeout<S, T> {
    inner: S,
    timeout: Option<Duration>,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for StallTimeout<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

impl<S, T> fmt::Debug for StallTimeout<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StallTimeout")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for StallTimeout<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Response = http::Response<TimeoutBody<B, T>>;
    type Error = HttpError;

    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let resp = self.inner.call(req).await?;
            let (parts, body) = resp.into_parts();
            let body = TimeoutBody::new(body, self.timeout, self.timer.clone());
            Ok(http::Response::from_parts(parts, body))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TimeoutBody;
    use crate::HttpError;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use oath_adapter_net_mock::MockTimer;
    use std::collections::VecDeque;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use crate::Guarded;
    use async_lock::Semaphore;
    use std::sync::Arc;

    // A body that never yields a frame — models a wedged/stalled transfer.
    struct PendingBody;
    impl Body for PendingBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Pending
        }
        fn is_end_stream(&self) -> bool {
            false
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }

    #[tokio::test]
    async fn stall_fires_when_no_frame_arrives() {
        let timer = MockTimer::new();
        let body = TimeoutBody::new(PendingBody, Some(Duration::from_secs(1)), timer.clone());
        let waiter = tokio::spawn(async move {
            let mut body = pin!(body);
            std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await
        });
        tokio::task::yield_now().await; // arms the deadline, inner pends
        timer.advance(Duration::from_secs(1)); // fire the stall deadline
        let frame = waiter.await.unwrap();
        assert!(
            matches!(frame, Some(Err(HttpError::Timeout))),
            "a stalled body must yield Timeout"
        );
    }

    // A multi-frame body; every frame is immediately ready.
    struct Frames {
        frames: VecDeque<Bytes>,
    }
    impl Frames {
        fn new<const N: usize>(frames: [&'static [u8]; N]) -> Self {
            Self {
                frames: frames.iter().copied().map(Bytes::from_static).collect(),
            }
        }
    }
    impl Body for Frames {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            let this = self.get_mut();
            Poll::Ready(this.frames.pop_front().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.frames.iter().map(|f| f.len() as u64).sum())
        }
    }

    #[test]
    fn each_frame_resets_the_deadline() {
        // timeout = 1s; two 500ms gaps (1s total) must NOT trip, because each
        // per-frame gap is < 1s. A no-reset impl (deadline armed once) WOULD trip
        // once cumulative time reaches 1s.
        let timer = MockTimer::new();
        let body = TimeoutBody::new(
            Frames::new([b"a", b"b"]),
            Some(Duration::from_secs(1)),
            timer.clone(),
        );
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Ok(_)))
        )); // frame a @ t=0
        timer.advance(Duration::from_millis(500));
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Ok(_)))
        )); // frame b @ t=0.5s
        timer.advance(Duration::from_millis(500)); // t=1.0s total
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(None)
        )); // end, NOT a stall
    }

    #[tokio::test]
    async fn none_timeout_is_inert() {
        // A None timeout never arms a deadline: a pending body just stays pending
        // even after the clock advances arbitrarily.
        let timer = MockTimer::new();
        let body = TimeoutBody::new(PendingBody, None, timer.clone());
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        timer.advance(Duration::from_secs(3600));
        assert!(
            body.as_mut().poll_frame(&mut cx).is_pending(),
            "None disables the stall timeout entirely"
        );
    }

    #[test]
    fn forwards_is_end_stream_and_size_hint() {
        let inner = Frames::new([b"ab", b"cde"]);
        let ref_hint = inner.size_hint().exact();
        let wrapped = TimeoutBody::new(
            Frames::new([b"ab", b"cde"]),
            Some(Duration::from_secs(1)),
            MockTimer::new(),
        );
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently unbounded
        assert!(!wrapped.is_end_stream());
        let ended = TimeoutBody::new(
            Frames::new([]),
            Some(Duration::from_secs(1)),
            MockTimer::new(),
        );
        assert!(ended.is_end_stream()); // forwarded, not the `false` default
    }

    // A stall must release the concurrency permit: with Guarded OUTSIDE TimeoutBody,
    // the stall error frame is a non-Ok frame that Guarded observes and releases on.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the test holds the guarded body across polls to prove release at the stall while still alive"
    )]
    #[test]
    fn stall_releases_the_concurrency_permit() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let timer = MockTimer::new();
        let inner = TimeoutBody::new(PendingBody, Some(Duration::from_secs(1)), timer.clone());
        let body = Guarded::new(inner, Some(permit));
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());

        assert!(body.as_mut().poll_frame(&mut cx).is_pending()); // arms deadline, permit held
        assert!(
            sem.try_acquire_arc().is_none(),
            "permit held while streaming"
        );

        timer.advance(Duration::from_secs(1)); // fire the stall
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Err(HttpError::Timeout)))
        ));
        assert!(
            sem.try_acquire_arc().is_some(),
            "the stall error must release the permit"
        );
    }

    use crate::Service;
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Layer;

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
    #[derive(Clone)]
    struct FastLeaf;
    impl Service<http::Request<Bytes>> for FastLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        async fn call(&self, _req: http::Request<Bytes>) -> Result<Self::Response, HttpError> {
            Ok(http::Response::new(StubBody {
                data: Some(Bytes::from_static(b"ok")),
            }))
        }
    }

    #[tokio::test]
    async fn layer_wraps_and_body_streams_through() {
        use super::StallTimeoutLayer;
        let svc =
            StallTimeoutLayer::new(Some(Duration::from_secs(30)), MockTimer::new()).layer(FastLeaf);
        let resp = svc
            .call(http::Request::new(Bytes::new()))
            .await
            .expect("fast leaf");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // wrapped, streamed through untouched
    }
}
