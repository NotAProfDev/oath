//! A response body that yields pre-set data frames, with a controllable
//! `size_hint`/`is_end_stream` for exercising body-metadata forwarding.

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use oath_adapter_net_http_api::HttpError;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A body that yields its configured frames in order, then ends.
#[derive(Debug, Default)]
pub struct MockBody {
    frames: VecDeque<Bytes>,
}

impl MockBody {
    /// A body yielding `frames` in order.
    #[must_use]
    pub fn new(frames: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            frames: frames.into_iter().collect(),
        }
    }

    /// An immediately-ended body.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

impl Body for MockBody {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        // `MockBody` holds no pinned fields, so `get_mut` is sound (auto-`Unpin`).
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

#[cfg(test)]
mod tests {
    use super::MockBody;
    use bytes::Bytes;
    use http_body::Body;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn yields_frames_then_ends_and_reports_exact_size() {
        let body = MockBody::new([Bytes::from_static(b"ab"), Bytes::from_static(b"cde")]);
        assert_eq!(body.size_hint().exact(), Some(5));
        assert!(!body.is_end_stream());
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected, Bytes::from_static(b"abcde"));
    }
}
