//! The named dependency-inversion seam the composition stack builds on.
//!
//! The `HttpClient` analogue for WS (ADR-0032 §7). The WS upgrade *is* an
//! HTTP GET, so the handshake is an `http::Request<()>` (body-agnostic parts
//! — the same shape the shared `AuthSource` will stamp, §8). Per ADR-0029 §5
//! it is a compile-time `impl WsConnector` seam — never `dyn`.
//!
//! This is the **composition** seam: the leaf and every inner resilience
//! layer implement it (ADR-0033 §1). The richer usage seam an adapter holds
//! (`ReconnectingConnector`/`ReconnectingConnection` + `WsControl`) is
//! produced only at the assembly boundary, in a later slice.

use crate::{Lifecycle, WsError, WsSink, WsSource};
use std::future::Future;

/// Establish WebSocket connections: one handshake in, three handles out.
pub trait WsConnector {
    /// The send half produced by a successful connect.
    type Sink: WsSink;
    /// The receive half produced by a successful connect.
    type Source: WsSource;

    /// Perform the upgrade handshake and yield the two single-owner halves
    /// plus the lifecycle read channel (ADR-0032 §2/§4). `&self` — a connector
    /// is shared; the reconnect layer calls it once per (re)connect.
    fn connect(
        &self,
        handshake: http::Request<()>,
    ) -> impl Future<Output = Result<(Self::Sink, Self::Source, Lifecycle), WsError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::WsConnector;
    use crate::{Frame, Lifecycle, LifecycleSnapshot, WsError, WsSink};
    use futures_core::Stream;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct StubSink;
    impl WsSink for StubSink {
        fn send(&mut self, _frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
        fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    struct StubSource;
    impl Stream for StubSource {
        type Item = Result<Frame, WsError>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    struct StubConnector;
    impl WsConnector for StubConnector {
        type Sink = StubSink;
        type Source = StubSource;
        #[allow(clippy::manual_async_fn)]
        fn connect(
            &self,
            _handshake: http::Request<()>,
        ) -> impl Future<Output = Result<(StubSink, StubSource, Lifecycle), WsError>> + Send
        {
            async {
                let (_tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
                Ok((StubSink, StubSource, lifecycle))
            }
        }
    }

    #[tokio::test]
    async fn connect_yields_the_three_handles() {
        fn assert_connector<C: WsConnector>(_: &C) {}
        assert_connector(&StubConnector);

        let mut handshake = http::Request::new(());
        *handshake.uri_mut() = "wss://api.ibkr.com/v1/api/ws".parse().unwrap();
        let (sink, _source, lifecycle) = StubConnector.connect(handshake).await.unwrap();
        assert_eq!(lifecycle.snapshot().epoch, 0);
        sink.close().await.unwrap();
    }
}
