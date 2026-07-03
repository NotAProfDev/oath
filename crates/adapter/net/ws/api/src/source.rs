//! The owned recv half: a stream of frames.
//!
//! Recv is `futures_core::Stream`, not a hand-rolled pull iterator — the
//! `http_body::Body` precedent (ADR-0032 §2): monomorphised, zero-box, and it
//! gives the resilience layers `StreamExt`/`unfold` instead of manual
//! `poll_next`. Exclusive `&mut` access is inherent in `Stream::poll_next`.

use crate::{Frame, WsError};
use futures_core::Stream;

/// The receive half of one WebSocket connection: frames in arrival order,
/// each `Ok(Frame)` or a terminal-ish `Err(WsError)`.
///
/// Blanket-implemented for every matching stream — a backend implements
/// `Stream` once and is a `WsSource` for free (the `HttpClient` move).
pub trait WsSource: Stream<Item = Result<Frame, WsError>> + Send {}

impl<S> WsSource for S where S: Stream<Item = Result<Frame, WsError>> + Send {}

#[cfg(test)]
mod tests {
    use super::WsSource;
    use crate::{Frame, WsError};
    use bytes::Bytes;
    use futures_core::Stream;
    use futures_util::StreamExt;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// Inline double: yields scripted items, then ends.
    struct ScriptSource {
        items: VecDeque<Result<Frame, WsError>>,
    }

    impl Stream for ScriptSource {
        type Item = Result<Frame, WsError>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            // No pinned fields — `get_mut` is sound (auto-`Unpin`).
            Poll::Ready(self.get_mut().items.pop_front())
        }
    }

    #[test]
    fn any_matching_stream_is_a_ws_source() {
        fn assert_ws_source<S: WsSource>(_: &S) {}
        let source = ScriptSource {
            items: VecDeque::new(),
        };
        assert_ws_source(&source); // blanket impl applies
    }

    #[tokio::test]
    async fn source_yields_items_then_ends() {
        let mut source = ScriptSource {
            items: VecDeque::from([
                Ok(Frame::Text(Bytes::from_static(b"{\"topic\":\"system\"}"))),
                Err(WsError::connection("reset")),
            ]),
        };
        assert!(source.next().await.unwrap().is_ok());
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());
    }
}
