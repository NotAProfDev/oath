//! The canonical HTTP response body and the per-request buffer/stream directive.

use crate::HttpError;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::Full;
use std::pin::Pin;
use std::task::{Context, Poll};

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

#[cfg(test)]
mod tests {
    use super::{BufferMode, ResponseBody};
    use crate::HttpError;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use std::pin::Pin;
    use std::task::{Context, Poll};

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
}
