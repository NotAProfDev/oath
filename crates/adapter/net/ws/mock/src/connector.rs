//! A scriptable `WsConnector` leaf.
//!
//! Each `connect` consumes the next script (a frame sequence, or a failure),
//! records the handshake, and exposes what every connection sent, whether it
//! closed, and its `LifecycleSender` so a test can drive lifecycle
//! transitions (ADR-0033 §9's "scripted frames + injectable disconnects and
//! `ErrorKind`s").

use crate::sink::ConnectionRecord;
use crate::{MockSink, MockSource, lock};
use oath_adapter_net_ws_api::{
    Frame, Lifecycle, LifecycleSender, LifecycleSnapshot, WsConnector, WsError,
};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
enum Script {
    /// The next connect succeeds; its source yields these items, then ends.
    Yield(VecDeque<Result<Frame, WsError>>),
    /// The next connect fails with this error.
    Fail(WsError),
}

#[derive(Debug, Default)]
struct State {
    scripts: VecDeque<Script>,
    handshakes: Vec<http::Request<()>>,
    records: Vec<Arc<Mutex<ConnectionRecord>>>,
}

/// A scriptable connector leaf that records everything.
///
/// Unscripted `connect`s succeed with an immediately-ended source. Successful
/// connections are numbered from 0 (the index used by the accessors), and
/// connection `n`'s lifecycle is seeded `LifecycleSnapshot::connected(n)`.
#[derive(Debug, Clone, Default)]
pub struct MockWsConnector {
    state: Arc<Mutex<State>>,
}

impl MockWsConnector {
    /// A connector with no scripts queued.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a successful connection whose source yields `items` in order,
    /// then ends. Inject a mid-stream disconnect by scripting an `Err` item.
    pub fn script_connection(&self, items: impl IntoIterator<Item = Result<Frame, WsError>>) {
        lock(&self.state)
            .scripts
            .push_back(Script::Yield(items.into_iter().collect()));
    }

    /// Enqueue a `connect` failure.
    pub fn script_connect_error(&self, error: WsError) {
        lock(&self.state).scripts.push_back(Script::Fail(error));
    }

    /// How many times `connect` was called (including failed attempts).
    #[must_use]
    pub fn connect_count(&self) -> usize {
        lock(&self.state).handshakes.len()
    }

    /// The handshake requests seen, in order (including failed attempts).
    #[must_use]
    pub fn recorded_handshakes(&self) -> Vec<http::Request<()>> {
        lock(&self.state).handshakes.clone()
    }

    /// Frames sent through successful connection `connection`'s sink, in
    /// order. Empty for an unknown index.
    #[must_use]
    pub fn sent_frames(&self, connection: usize) -> Vec<Frame> {
        lock(&self.state)
            .records
            .get(connection)
            .map(|record| lock(record).sent.clone())
            .unwrap_or_default()
    }

    /// Whether successful connection `connection`'s sink was closed.
    #[must_use]
    pub fn close_called(&self, connection: usize) -> bool {
        lock(&self.state)
            .records
            .get(connection)
            .is_some_and(|record| lock(record).closed)
    }

    /// Take successful connection `connection`'s lifecycle write side, to
    /// drive transitions from the test. `None` for an unknown index or if
    /// already taken.
    #[must_use]
    pub fn take_lifecycle_sender(&self, connection: usize) -> Option<LifecycleSender> {
        lock(&self.state)
            .records
            .get(connection)
            .and_then(|record| lock(record).lifecycle.take())
    }
}

impl WsConnector for MockWsConnector {
    type Sink = MockSink;
    type Source = MockSource;

    fn connect(
        &self,
        handshake: http::Request<()>,
    ) -> impl Future<Output = Result<(MockSink, MockSource, Lifecycle), WsError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut state = lock(&state);
            state.handshakes.push(handshake);
            let items = match state.scripts.pop_front() {
                Some(Script::Fail(error)) => return Err(error),
                Some(Script::Yield(items)) => items,
                None => VecDeque::new(),
            };
            let epoch = u64::try_from(state.records.len()).unwrap_or(u64::MAX);
            let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(epoch));
            let record = Arc::new(Mutex::new(ConnectionRecord {
                lifecycle: Some(tx),
                ..ConnectionRecord::default()
            }));
            state.records.push(Arc::clone(&record));
            drop(state);
            Ok((MockSink::new(record), MockSource::new(items), lifecycle))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MockWsConnector;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};
    use oath_adapter_net_ws_api::{
        ConnState, Frame, LifecycleSnapshot, WsConnector, WsError, WsSink,
    };

    fn handshake(uri: &str) -> http::Request<()> {
        let mut req = http::Request::new(());
        *req.uri_mut() = uri.parse().unwrap();
        req
    }

    #[tokio::test]
    async fn scripted_frames_flow_and_everything_is_recorded() {
        let connector = MockWsConnector::new();
        connector.script_connection([
            Ok(Frame::Text(Bytes::from_static(
                b"{\"topic\":\"smd+265598\"}",
            ))),
            Err(WsError::connection("reset")),
        ]);

        let (mut sink, mut source, lifecycle) = connector
            .connect(handshake("wss://api.ibkr.com/v1/api/ws"))
            .await
            .unwrap();

        assert_eq!(
            lifecycle.snapshot().phase,
            ConnState::Connected { epoch: 0 }
        );
        assert!(source.next().await.unwrap().is_ok());
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());

        sink.send(Frame::Text(Bytes::from_static(b"smd+265598+{}")))
            .await
            .unwrap();
        sink.close().await.unwrap();

        assert_eq!(connector.connect_count(), 1);
        assert_eq!(
            connector.recorded_handshakes()[0].uri(),
            "wss://api.ibkr.com/v1/api/ws"
        );
        assert_eq!(
            connector.sent_frames(0),
            vec![Frame::Text(Bytes::from_static(b"smd+265598+{}"))]
        );
        assert!(connector.close_called(0));
    }

    #[tokio::test]
    async fn scripted_connect_error_fails_then_unscripted_connect_succeeds_empty() {
        let connector = MockWsConnector::new();
        connector.script_connect_error(WsError::auth("session expired"));

        let err = connector
            .connect(handshake("wss://x/ws"))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
        assert_eq!(connector.connect_count(), 1); // the failed attempt is counted

        // No script left: connect succeeds with an immediately-ended source,
        // and epochs number successful connections (this is epoch 0).
        let (_sink, mut source, lifecycle) =
            connector.connect(handshake("wss://x/ws")).await.unwrap();
        assert_eq!(lifecycle.snapshot().epoch, 0);
        assert!(source.next().await.is_none());
    }

    #[tokio::test]
    async fn taken_lifecycle_sender_drives_transitions() {
        let connector = MockWsConnector::new();
        let (_sink, _source, mut lifecycle) =
            connector.connect(handshake("wss://x/ws")).await.unwrap();

        let tx = connector.take_lifecycle_sender(0).unwrap();
        assert!(connector.take_lifecycle_sender(0).is_none()); // taken once

        tx.send(LifecycleSnapshot {
            phase: ConnState::Stale,
            ..LifecycleSnapshot::connected(0)
        });
        assert!(lifecycle.changed().await);
        assert_eq!(lifecycle.snapshot().phase, ConnState::Stale);
    }
}
