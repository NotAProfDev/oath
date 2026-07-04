//! The WebSocket frame vocabulary — the leaf/inter-layer unit of transport.
//!
//! "Untyped" (ADR-0032 §1) means *no venue/JSON typing* — not flattening
//! WebSocket's own protocol frame kinds, which are transport concerns
//! (RFC 6455), not venue concerns (§3). After the ADR-0033 default stack the
//! adapter-facing source delivers only `Text`/`Binary` data frames; control
//! frames are absorbed by the heartbeat layer and bypass the data buffer.

use bytes::Bytes;

/// The payload of a `Close` frame: an RFC 6455 close code plus a UTF-8 reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    /// The RFC 6455 §7.4 close code (e.g. `1000` normal closure).
    pub code: u16,
    /// The UTF-8 close reason; may be empty.
    pub reason: String,
}

/// One WebSocket frame. Payloads are raw [`Bytes`] — the transport never
/// parses them (grammar-blindness, ADR-0032 §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A UTF-8 text data frame (kept as bytes; the adapter owns parsing).
    Text(Bytes),
    /// A binary data frame.
    Binary(Bytes),
    /// A protocol ping control frame.
    Ping(Bytes),
    /// A protocol pong control frame.
    Pong(Bytes),
    /// A close control frame with an optional code + reason.
    Close(Option<CloseFrame>),
}

impl Frame {
    /// Whether this is a data frame (`Text`/`Binary`) — what the default
    /// stack delivers to the adapter (ADR-0032 §3).
    #[must_use]
    pub const fn is_data(&self) -> bool {
        matches!(self, Self::Text(_) | Self::Binary(_))
    }

    /// Whether this is a control frame (`Ping`/`Pong`/`Close`) — absorbed by
    /// the heartbeat layer; bypasses the data buffer (ADR-0032 §3/§6).
    #[must_use]
    pub const fn is_control(&self) -> bool {
        !self.is_data()
    }
}

#[cfg(test)]
mod tests {
    use super::{CloseFrame, Frame};
    use bytes::Bytes;

    #[test]
    fn data_and_control_frames_classify() {
        assert!(Frame::Text(Bytes::from_static(b"{}")).is_data());
        assert!(Frame::Binary(Bytes::new()).is_data());
        assert!(Frame::Ping(Bytes::new()).is_control());
        assert!(Frame::Pong(Bytes::new()).is_control());
        assert!(Frame::Close(None).is_control());
    }

    #[test]
    fn frames_are_cloneable_and_comparable() {
        let close = Frame::Close(Some(CloseFrame {
            code: 1000,
            reason: "bye".to_owned(),
        }));
        assert_eq!(close.clone(), close);
        assert_ne!(close, Frame::Close(None));
    }
}
