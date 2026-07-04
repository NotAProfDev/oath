//! The owned send half: one-shot RPITIT `send`, terminal `close`.
//!
//! Deliberately **not** `futures::Sink` — its `poll_ready`/`start_send`/
//! `poll_flush` is the poll-handshake the `Service` design walked away from
//! (ADR-0032 §2); subscribe/heartbeat traffic is low-volume, so a one-shot
//! `send` suffices. The half is single-owner and moves to its own task.

use crate::{Frame, WsError};
use std::future::Future;

/// The send half of one WebSocket connection.
///
/// `Send` because the halves move to separate tasks (concurrent send of
/// subscribe/heartbeat vs. receive of frames). `'static` is not required
/// here — it is enforced at the composition boundary, as for `Service`.
pub trait WsSink: Send {
    /// Send one frame.
    fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send;

    /// Initiate the closing handshake. Consumes the sink — shutdown is one-way
    /// and terminal, so the sink cannot be used after close is requested
    /// (enforced by the type system, ADR-0032 §2).
    fn close(self) -> impl Future<Output = Result<(), WsError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::WsSink;
    use crate::{Frame, WsError};
    use bytes::Bytes;
    use std::future::Future;

    /// Inline double: records sent frames, succeeds on close.
    #[derive(Default)]
    struct VecSink {
        sent: Vec<Frame>,
    }

    impl WsSink for VecSink {
        fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
            self.sent.push(frame);
            std::future::ready(Ok(()))
        }

        fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn send_is_one_shot_and_close_consumes_the_sink() {
        let mut sink = VecSink::default();
        sink.send(Frame::Text(Bytes::from_static(b"smd+265598+{}")))
            .await
            .unwrap();
        assert_eq!(sink.sent.len(), 1);
        // `close(self)` takes the sink by value — `sink.send(...)` after this
        // line would be a compile error (ADR-0032 §2's type-system guarantee).
        sink.close().await.unwrap();
    }
}
