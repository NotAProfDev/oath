//! The send half a [`crate::MockWsConnector`] yields: records every frame and
//! the close into the per-connection record the connector shares.

use crate::lock;
use oath_adapter_net_ws_api::{Frame, LifecycleSender, WsError, WsSink};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// What one mock connection observed. Shared between the [`MockSink`] handed
/// to the code under test and the [`crate::MockWsConnector`]'s accessors.
#[derive(Debug, Default)]
pub(crate) struct ConnectionRecord {
    /// Frames sent through the sink, in order.
    pub(crate) sent: Vec<Frame>,
    /// Whether `close` was called.
    pub(crate) closed: bool,
    /// The lifecycle write side, until a test `take`s it.
    pub(crate) lifecycle: Option<LifecycleSender>,
}

/// A recording send half. Sends always succeed.
#[derive(Debug)]
pub struct MockSink {
    record: Arc<Mutex<ConnectionRecord>>,
}

impl MockSink {
    pub(crate) const fn new(record: Arc<Mutex<ConnectionRecord>>) -> Self {
        Self { record }
    }
}

impl WsSink for MockSink {
    fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
        lock(&self.record).sent.push(frame);
        std::future::ready(Ok(()))
    }

    fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
        lock(&self.record).closed = true;
        std::future::ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionRecord, MockSink};
    use bytes::Bytes;
    use oath_adapter_net_ws_api::{Frame, WsSink};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn records_sends_and_close() {
        let record = Arc::new(Mutex::new(ConnectionRecord::default()));
        let mut sink = MockSink::new(Arc::clone(&record));
        sink.send(Frame::Text(Bytes::from_static(b"tic")))
            .await
            .unwrap();
        sink.close().await.unwrap();
        let guard = record.lock().unwrap();
        assert_eq!(guard.sent, vec![Frame::Text(Bytes::from_static(b"tic"))]);
        assert!(guard.closed);
        drop(guard);
    }
}
