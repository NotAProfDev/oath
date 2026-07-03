//! The receive half a [`crate::MockWsConnector`] yields: pops its scripted
//! items in order, then ends the stream (connection over).

use futures_core::Stream;
use oath_adapter_net_ws_api::{Frame, WsError};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A scripted receive half. Satisfies `WsSource` via the blanket impl.
#[derive(Debug)]
pub struct MockSource {
    items: VecDeque<Result<Frame, WsError>>,
}

impl MockSource {
    /// A source that yields `items` in order, then ends.
    #[must_use]
    pub const fn new(items: VecDeque<Result<Frame, WsError>>) -> Self {
        Self { items }
    }
}

impl Stream for MockSource {
    type Item = Result<Frame, WsError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // No pinned fields — `get_mut` is sound (auto-`Unpin`).
        Poll::Ready(self.get_mut().items.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::MockSource;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use oath_adapter_net_ws_api::{Frame, WsError, WsSource};
    use std::collections::VecDeque;

    #[tokio::test]
    async fn yields_scripted_items_in_order_then_ends() {
        fn assert_ws_source<S: WsSource>(_: &S) {}
        let mut source = MockSource::new(VecDeque::from([
            Ok(Frame::Text(Bytes::from_static(
                b"{\"topic\":\"system\",\"hb\":1}",
            ))),
            Err(WsError::connection("reset")),
        ]));
        assert_ws_source(&source);
        assert_eq!(
            source.next().await.unwrap().unwrap(),
            Frame::Text(Bytes::from_static(b"{\"topic\":\"system\",\"hb\":1}"))
        );
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());
    }
}
