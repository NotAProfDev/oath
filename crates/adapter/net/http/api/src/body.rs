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
    /// The canonical response body: one buffered frame *xor* a live streaming
    /// body, behind one stable type so adapters never name the buffer-vs-stream
    /// machinery. Forwards all three `Body` methods to the active arm — a
    /// wrapper that silently reported the default `size_hint`/`is_end_stream`
    /// would make a caller's `.collect()` pre-size and any max-size guard wrong.
    #[project = ResponseBodyProj]
    #[allow(missing_docs)]
    pub enum ResponseBody<B> {
        /// A fully-collected body (single frame).
        Buffered { #[pin] body: Full<Bytes> },
        /// A live streaming backend body.
        Streaming { #[pin] body: B },
    }
}

impl<B> ResponseBody<B> {
    /// Wrap already-collected bytes as a one-frame buffered body.
    #[must_use]
    pub fn buffered(bytes: Bytes) -> Self {
        Self::Buffered {
            body: Full::new(bytes),
        }
    }

    /// Wrap a live streaming backend body.
    #[must_use]
    pub const fn streaming(body: B) -> Self {
        Self::Streaming { body }
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
        match self.project() {
            // `Full`'s error is `Infallible`, so map it away to unify with `HttpError`.
            ResponseBodyProj::Buffered { body } => body
                .poll_frame(cx)
                .map(|frame| frame.map(|res| res.map_err(|never| match never {}))),
            ResponseBodyProj::Streaming { body } => body.poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Buffered { body } => body.is_end_stream(),
            Self::Streaming { body } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Buffered { body } => body.size_hint(),
            Self::Streaming { body } => body.size_hint(),
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
    #[must_use]
    pub const fn new(inner: B, permit: Option<SemaphoreGuardArc>) -> Self {
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

    // `significant_drop_tightening` false-positives on the `pin_project!`
    // projection: it treats the borrowed `permit: &mut Option<SemaphoreGuardArc>`
    // field as if `this` owned the guard's `Drop`, and its own suggested fix
    // (dropping `this` before `*this.permit = None`) would not compile.
    #[allow(clippy::significant_drop_tightening)]
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        if frame.is_none() {
            // Eager release at stream-end: a fully-read but still-held body
            // must not keep hold of one of the venue's concurrency slots.
            // Dropping the guard is synchronous and runtime-free.
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

#[cfg(test)]
mod tests {
    use super::{BufferMode, Guarded, ResponseBody};
    use crate::HttpError;
    use async_lock::Semaphore;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
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

    // See the `poll_frame` justification above: `significant_drop_tightening`
    // false-positives on `Guarded`'s `Option<SemaphoreGuardArc>` field even
    // when it's `None`.
    #[allow(clippy::significant_drop_tightening)]
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
    #[allow(clippy::significant_drop_tightening)]
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
    #[allow(clippy::significant_drop_tightening)]
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
}
