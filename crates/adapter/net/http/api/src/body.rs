//! The canonical HTTP response body, the per-request buffer/stream directive,
//! and the permit-carrying `Guarded` wrapper.

use crate::HttpError;
use async_lock::SemaphoreGuardArc;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::Full;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

/// Per-request directive: buffer the response body inside the retry boundary, or
/// return it streaming at headers (ADR-0030 §4). `Copy` so it survives the
/// request clone `Retry` makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMode {
    /// Collect the body to `Bytes` before returning (full retry coverage).
    Buffer,
    /// Return the live body at headers (adapter owns mid-stream recovery).
    Stream,
}

pin_project_lite::pin_project! {
    /// The canonical HTTP response body: one buffered frame *xor* a live streaming
    /// body, behind one **opaque** type so adapters never name — or match on — the
    /// buffer-vs-stream machinery (ADR-0030 §3). Construct via
    /// [`buffered`](Self::buffered)/[`streaming`](Self::streaming), inspect the arm
    /// via [`is_buffered`](Self::is_buffered)/[`is_streaming`](Self::is_streaming),
    /// and consume it through the [`Body`] trait. Forwards all three `Body` methods
    /// to the active arm — a wrapper that silently reported the default
    /// `size_hint`/`is_end_stream` would make a caller's `.collect()` pre-size and
    /// any max-size guard wrong.
    pub struct ResponseBody<B> {
        #[pin]
        inner: Inner<B>,
    }
}

pin_project_lite::pin_project! {
    /// The private buffer-xor-stream representation `ResponseBody` hides so callers
    /// cannot construct or destructure the internal arms (M9, ADR-0030 §3).
    #[project = InnerProj]
    enum Inner<B> {
        Buffered { #[pin] body: Full<Bytes> },
        Streaming { #[pin] body: B },
    }
}

impl<B> ResponseBody<B> {
    /// Wrap already-collected bytes as a one-frame buffered body.
    #[must_use]
    pub fn buffered(bytes: Bytes) -> Self {
        Self {
            inner: Inner::Buffered {
                body: Full::new(bytes),
            },
        }
    }

    /// Wrap a live streaming backend body.
    #[must_use]
    pub const fn streaming(body: B) -> Self {
        Self {
            inner: Inner::Streaming { body },
        }
    }

    /// Whether this is a fully-collected (buffered) body.
    #[must_use]
    pub const fn is_buffered(&self) -> bool {
        matches!(self.inner, Inner::Buffered { .. })
    }

    /// Whether this is a live streaming body.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        matches!(self.inner, Inner::Streaming { .. })
    }
}

impl<B> fmt::Debug for ResponseBody<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Opaque by design: report only which arm is active, never the payload.
        f.debug_struct("ResponseBody")
            .field(
                "kind",
                &if self.is_buffered() {
                    "buffered"
                } else {
                    "streaming"
                },
            )
            .finish_non_exhaustive()
    }
}

impl<B> Body for ResponseBody<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        match self.project().inner.project() {
            // `Full`'s error is `Infallible`, so map it away to unify with `HttpError`.
            InnerProj::Buffered { body } => body
                .poll_frame(cx)
                .map(|frame| frame.map(|res| res.map_err(|never| match never {}))),
            InnerProj::Streaming { body } => body.poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.inner {
            Inner::Buffered { body } => body.is_end_stream(),
            Inner::Streaming { body } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.inner {
            Inner::Buffered { body } => body.size_hint(),
            Inner::Streaming { body } => body.size_hint(),
        }
    }
}

pin_project_lite::pin_project! {
    /// Wraps a response body, carrying an optional concurrency permit released
    /// at the **earlier of stream-end or drop** (ADR-0031 §3 as amended by
    /// ADR-0034). `RateLimit` always returns `http::Response<Guarded<B>>`:
    /// `permit: None` for rate-scoped, unscoped, or buffered responses;
    /// `permit: Some(_)` only for a streaming concurrency-scoped response, so
    /// the permit rides the transfer instead of freeing at headers.
    pub struct Guarded<B> {
        #[pin]
        inner: B,
        permit: Option<SemaphoreGuardArc>,
    }
}

impl<B> Guarded<B> {
    /// Wrap `inner`, optionally carrying a concurrency `permit`.
    ///
    /// If `inner` is already ended (`is_end_stream()` is `true`), the permit is
    /// released immediately: stream-end is *now*, so holding it would waste a
    /// concurrency slot. A consumer is allowed to observe `is_end_stream()` and
    /// never call `poll_frame` (per `http-body`), so the eager release in
    /// `poll_frame` cannot be relied on for an already-ended body.
    #[must_use]
    pub fn new(inner: B, permit: Option<SemaphoreGuardArc>) -> Self
    where
        B: Body,
    {
        let permit = if inner.is_end_stream() { None } else { permit };
        Self { inner, permit }
    }
}

impl<B: fmt::Debug> fmt::Debug for Guarded<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guarded")
            .field("inner", &self.inner)
            .field("permit_held", &self.permit.is_some())
            .finish()
    }
}

impl<B> Body for Guarded<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    // `significant_drop_tightening` correctly flags the deliberately-held guard in the
    // `pin_project!` projection: the borrowed `permit: &mut Option<SemaphoreGuardArc>`
    // field must stay alive until the terminal frame. The lint's drop-sooner fix is wrong here —
    // the permit must live until `*this.permit = None` at stream termination.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "permit is deliberately held until the terminal frame, then released via `*this.permit = None`; the lint's drop-sooner fix is wrong here"
    )]
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        if !matches!(frame, Some(Ok(_))) {
            // Eager release at stream termination — the terminal `None` frame
            // *or* an error frame (after which the body is not polled again).
            // A still-held body must not keep one of the venue's concurrency
            // slots; dropping the guard is synchronous and runtime-free.
            *this.permit = None;
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

pin_project_lite::pin_project! {
    /// Wraps a response body, failing with [`HttpError::BodyTooLarge`] once the
    /// cumulative DATA-frame bytes exceed `remaining`. A **typed** alternative to
    /// `http_body_util::Limited` (which boxes its error): the whole HTTP stack
    /// keeps one concrete `HttpError` for service *and* body. Forwards
    /// `is_end_stream`; clamps `size_hint` to `remaining` (ADR-0034 §2), so a
    /// downstream collector stays bounded.
    pub struct LimitedBody<B> {
        #[pin]
        inner: B,
        remaining: u64,
    }
}

impl<B> LimitedBody<B> {
    /// Wrap `inner`, rejecting once cumulative DATA bytes exceed `max_bytes`.
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::LimitedBody;
    /// use http_body_util::Empty;
    /// use bytes::Bytes;
    ///
    /// let _body = LimitedBody::new(Empty::<Bytes>::new(), 16 * 1024 * 1024);
    /// ```
    #[must_use]
    pub const fn new(inner: B, max_bytes: u64) -> Self {
        Self {
            inner,
            remaining: max_bytes,
        }
    }
}

impl<B: fmt::Debug> fmt::Debug for LimitedBody<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedBody")
            .field("inner", &self.inner)
            .field("remaining", &self.remaining)
            .finish()
    }
}

impl<B> Body for LimitedBody<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    let len = data.len() as u64;
                    if len > *this.remaining {
                        *this.remaining = 0;
                        return Poll::Ready(Some(Err(HttpError::BodyTooLarge)));
                    }
                    *this.remaining -= len;
                }
                Poll::Ready(Some(Ok(frame)))
            },
            // Terminal None or an inner error: pass through unchanged.
            other => Poll::Ready(other),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        // Mirror http_body_util::Limited's clamp so lower never exceeds upper.
        let mut hint = self.inner.size_hint();
        let n = self.remaining;
        if hint.lower() >= n {
            hint.set_exact(n);
        } else if let Some(max) = hint.upper() {
            hint.set_upper(n.min(max));
        } else {
            hint.set_upper(n);
        }
        hint
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferMode, Guarded, LimitedBody, ResponseBody};
    use crate::HttpError;
    use async_lock::Semaphore;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use std::collections::VecDeque;
    use std::pin::{Pin, pin};
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    // Inner body with a known, non-default size_hint / is_end_stream, so the
    // parity assertion is meaningful.
    struct Stub {
        remaining: u64,
    }
    impl Body for Stub {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(None)
        }
        fn is_end_stream(&self) -> bool {
            self.remaining == 0
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.remaining)
        }
    }

    #[test]
    fn streaming_forwards_size_hint_and_is_end_stream() {
        let reference = Stub { remaining: 42 };
        let ref_hint = reference.size_hint().exact();
        let ref_end = reference.is_end_stream();
        let wrapped = ResponseBody::streaming(Stub { remaining: 42 });
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently None/unbounded
        assert_eq!(wrapped.is_end_stream(), ref_end);
    }

    #[test]
    fn streaming_is_end_stream_is_forwarded_not_defaulted() {
        // Inner `is_end_stream()` is `true`; the trait default is `false`, so this
        // assertion fails if the override were dropped — unlike a `remaining: 42`
        // (false) case, which the default would also satisfy.
        let wrapped = ResponseBody::streaming(Stub { remaining: 0 });
        assert!(wrapped.is_end_stream());
    }

    #[test]
    fn buffered_reports_exact_length() {
        let body: ResponseBody<Stub> = ResponseBody::buffered(Bytes::from_static(b"hello"));
        assert_eq!(body.size_hint().exact(), Some(5));
    }

    #[test]
    fn buffer_mode_is_copy() {
        let m = BufferMode::Buffer;
        let n = m; // Copy
        assert_eq!(m, n);
    }

    /// Multi-frame inner body (inline double — no dev-dep on net-http-mock,
    /// same no-cycle choice as PR 2).
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
            Poll::Ready(this.frames.pop_front().map(|data| Ok(Frame::data(data))))
        }

        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            let total: u64 = self.frames.iter().map(|f| f.len() as u64).sum();
            SizeHint::with_exact(total)
        }
    }

    // The lint correctly flags the deliberately-held guard; we suppress it here because
    // the test must hold the guarded body across assertions to verify release behavior.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "holds the guarded body across assertions"
    )]
    #[test]
    fn guarded_forwards_size_hint_and_is_end_stream() {
        let reference = Frames::new([b"ab", b"cde"]);
        let ref_hint = reference.size_hint().exact();
        let wrapped = Guarded::new(Frames::new([b"ab", b"cde"]), None);
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently unbounded
        assert!(!wrapped.is_end_stream());
        let ended = Guarded::new(Frames::new([]), None);
        assert!(ended.is_end_stream()); // forwarded, not the `false` default
    }

    // Keeping `body`/the permit alive across the assertions is the point of
    // this test (proving eager release *while still alive*), so
    // `significant_drop_tightening`'s "drop it sooner" suggestion is exactly
    // what must NOT happen here.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the test holds the guarded body across assertions to prove release at stream-end while still alive"
    )]
    #[test]
    fn permit_releases_on_terminal_frame_before_drop() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let body = Guarded::new(Frames::new([b"a"]), Some(permit));
        assert!(sem.try_acquire_arc().is_none(), "permit held while unread");

        let mut cx = Context::from_waker(Waker::noop());
        let mut body = pin!(body);
        // Drain: data frame, then the terminal `None` (which must release).
        while let Poll::Ready(Some(res)) = body.as_mut().poll_frame(&mut cx) {
            res.expect("data frame");
        }
        // `body` is still alive here — the release was eager, not drop-driven.
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released at the terminal frame"
        );
    }

    // Same rationale as `permit_releases_on_terminal_frame_before_drop`: the
    // scoped block controls exactly when `body` drops, which is the behavior
    // under test.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the scoped block controls exactly when the body drops — the behavior under test"
    )]
    #[test]
    fn permit_releases_on_early_drop() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        {
            let body = Guarded::new(Frames::new([b"a", b"b"]), Some(permit));
            let mut cx = Context::from_waker(Waker::noop());
            let mut body = pin!(body);
            // Read only the first frame — abort mid-stream.
            assert!(matches!(
                body.as_mut().poll_frame(&mut cx),
                Poll::Ready(Some(Ok(_)))
            ));
            assert!(sem.try_acquire_arc().is_none(), "still held mid-stream");
        }
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released on early drop"
        );
    }

    #[test]
    fn permit_releases_when_body_is_already_ended_at_construction() {
        // A consumer may observe `is_end_stream()` and never poll (legal per
        // `http-body`), so the eager release must happen at construction for an
        // already-ended body — not wait for a `poll_frame` that never comes.
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let _body = Guarded::new(Frames::new([]), Some(permit));
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released for an already-ended body, without polling"
        );
    }

    /// Yields one error frame, then ends. `is_end_stream()` is `false` until the
    /// error is emitted, so `Guarded::new` keeps the permit and the release is
    /// exercised through `poll_frame`.
    struct ErrorThenEnd {
        emitted: bool,
    }

    impl Body for ErrorThenEnd {
        type Data = Bytes;
        type Error = HttpError;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            let this = self.get_mut();
            if this.emitted {
                Poll::Ready(None)
            } else {
                this.emitted = true;
                Poll::Ready(Some(Err(HttpError::other("boom"))))
            }
        }

        fn is_end_stream(&self) -> bool {
            self.emitted
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the body holds the guard across assertions to prove release on the error frame while still alive"
    )]
    #[test]
    fn permit_releases_on_error_frame_before_drop() {
        // An error frame practically ends the stream (the body is not polled
        // again), so the permit must release then — not linger until `Drop`.
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let body = Guarded::new(ErrorThenEnd { emitted: false }, Some(permit));
        assert!(
            sem.try_acquire_arc().is_none(),
            "permit held before the error"
        );

        let mut cx = Context::from_waker(Waker::noop());
        let mut body = pin!(body);
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Err(_)))
        ));
        // `body` is still alive — release was eager on the error, not drop-driven.
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released on the error frame"
        );
    }

    #[tokio::test]
    async fn limited_body_passes_under_cap() {
        let body = LimitedBody::new(Frames::new([b"ab", b"cde"]), 10);
        let bytes = body.collect().await.unwrap().to_bytes();
        assert_eq!(bytes, Bytes::from_static(b"abcde"));
    }

    #[tokio::test]
    async fn limited_body_passes_at_exact_cap() {
        // 2 + 3 == 5; each frame's len is not > remaining, so the boundary passes.
        let body = LimitedBody::new(Frames::new([b"ab", b"cde"]), 5);
        let bytes = body.collect().await.unwrap().to_bytes();
        assert_eq!(bytes, Bytes::from_static(b"abcde"));
    }

    #[tokio::test]
    async fn limited_body_errors_over_cap() {
        // cap 4: "abc" (3) ok → remaining 1; "def" (3) > 1 → BodyTooLarge.
        let body = LimitedBody::new(Frames::new([b"abc", b"def"]), 4);
        let err = body.collect().await.expect_err("must overflow");
        assert!(matches!(err, HttpError::BodyTooLarge));
    }

    #[test]
    fn limited_body_clamps_size_hint_and_forwards_is_end_stream() {
        // inner exact = 100, cap 10 → clamp to exact 10 (lower >= remaining path).
        let wrapped = LimitedBody::new(Stub { remaining: 100 }, 10);
        assert_eq!(wrapped.size_hint().exact(), Some(10));
        assert!(!wrapped.is_end_stream());
        let ended = LimitedBody::new(Stub { remaining: 0 }, 10);
        assert!(ended.is_end_stream()); // forwarded
    }

    /// Inner body reporting a RANGE size hint (`lower != upper`), unlike
    /// `Stub`'s exact hint — needed to exercise `LimitedBody::size_hint`'s
    /// `lower < remaining` branches, which an exact hint can never reach.
    struct RangeHint {
        lower: u64,
        upper: Option<u64>,
    }

    impl Body for RangeHint {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(None)
        }
        fn size_hint(&self) -> SizeHint {
            let mut hint = SizeHint::default();
            hint.set_lower(self.lower);
            if let Some(upper) = self.upper {
                hint.set_upper(upper);
            }
            hint
        }
    }

    #[test]
    fn limited_body_clamps_size_hint_upper_when_bounded_above_cap() {
        // inner lower(2) < cap(10) and inner upper(50) > cap: the
        // `lower < remaining` branch with `Some(max)` must clamp upper to
        // `remaining.min(max)` == 10, not leave it at the inner's 50.
        let wrapped = LimitedBody::new(
            RangeHint {
                lower: 2,
                upper: Some(50),
            },
            10,
        );
        let hint = wrapped.size_hint();
        assert_eq!(hint.upper(), Some(10));
        assert!(hint.lower() <= 10);
    }

    #[test]
    fn limited_body_clamps_size_hint_upper_when_inner_unbounded() {
        // inner lower(2) < cap(10) and inner upper is `None` (unbounded): the
        // final `else` branch must set upper to `remaining` == 10, not leave
        // it unbounded.
        let wrapped = LimitedBody::new(
            RangeHint {
                lower: 2,
                upper: None,
            },
            10,
        );
        assert_eq!(wrapped.size_hint().upper(), Some(10));
    }

    /// Yields a trailers frame (no `data_ref()`), then ends. Used to prove
    /// `LimitedBody` only counts DATA-frame bytes toward `remaining` — a
    /// trailers frame must pass through without consuming budget or erroring.
    struct TrailersOnly {
        sent: bool,
    }

    impl Body for TrailersOnly {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            let this = self.get_mut();
            if this.sent {
                Poll::Ready(None)
            } else {
                this.sent = true;
                Poll::Ready(Some(Ok(Frame::trailers(http::HeaderMap::new()))))
            }
        }
        fn is_end_stream(&self) -> bool {
            self.sent
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }

    #[tokio::test]
    async fn limited_body_does_not_count_trailers_toward_cap() {
        // cap 0: if the trailers' bytes were (wrongly) counted, any nonzero
        // count would overflow a zero cap and this would error. A tiny cap
        // with a non-DATA-only frame is the sharpest regression trap.
        let body = LimitedBody::new(TrailersOnly { sent: false }, 0);
        let collected = body.collect().await.expect("trailers must not error");
        assert!(collected.trailers().is_some());
    }
}
