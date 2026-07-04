//! The single concrete error type across the WS stack — transport and
//! middleware failures only.
//!
//! Venue-level errors arriving as data frames are NOT errors here: they flow
//! through the source as `Text`/`Binary` for the adapter to classify
//! (ADR-0032 §1). `WsError` implements [`HasErrorKind`] once; the resilience
//! layers branch only on [`ErrorKind`] (ADR-0033 §7).

use crate::CloseFrame;
use oath_adapter_net_api::{ErrorKind, HasErrorKind};

/// A boxed error source, preserving backend detail for logs without leaking
/// the concrete type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The single error of the WS transport: `connect`, `send`, `close`, and every
/// source item use it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsError {
    /// The operation did not complete within its timeout.
    #[error("operation timed out")]
    Timeout,
    /// A connection-level failure (DNS, TCP, TLS, WS handshake or protocol).
    #[error("connection failure")]
    Connection(#[source] BoxError),
    /// Credential stamping or refresh failed.
    #[error("authorization failed: {0}")]
    Auth(String),
    /// The peer closed the connection (close frame, possibly with code+reason).
    #[error("connection closed by peer{}", .0.as_ref().map_or_else(
        String::new,
        |frame| format!(" (code {}, reason {:?})", frame.code, frame.reason),
    ))]
    Closed(Option<CloseFrame>),
    /// A backend error that does not fit another variant.
    #[error("websocket error")]
    Other(#[source] BoxError),
}

impl WsError {
    /// Construct a [`WsError::Connection`] from a source error.
    #[must_use]
    pub fn connection(source: impl Into<BoxError>) -> Self {
        Self::Connection(source.into())
    }

    /// Construct a [`WsError::Auth`] from a message.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    /// Construct a [`WsError::Other`] from a source error.
    #[must_use]
    pub fn other(source: impl Into<BoxError>) -> Self {
        Self::Other(source.into())
    }
}

impl HasErrorKind for WsError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Timeout => ErrorKind::Timeout,
            // A peer close is a connection-level loss for retry purposes;
            // per-close-code refinement is the adapter's classification hook
            // (ADR-0033 §7), not a transport concern.
            Self::Connection(_) | Self::Closed(_) => ErrorKind::Connection,
            Self::Auth(_) => ErrorKind::Auth,
            Self::Other(_) => ErrorKind::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WsError;
    use crate::CloseFrame;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(WsError::Timeout.kind(), ErrorKind::Timeout);
        assert_eq!(WsError::connection("reset").kind(), ErrorKind::Connection);
        assert_eq!(WsError::auth("expired").kind(), ErrorKind::Auth);
        assert_eq!(WsError::Closed(None).kind(), ErrorKind::Connection);
        let close = CloseFrame {
            code: 1000,
            reason: String::new(),
        };
        assert_eq!(WsError::Closed(Some(close)).kind(), ErrorKind::Connection);
        assert_eq!(WsError::other("boom").kind(), ErrorKind::Unknown);
    }

    #[test]
    fn auth_carries_message() {
        assert_eq!(
            WsError::auth("session expired").to_string(),
            "authorization failed: session expired"
        );
    }

    #[test]
    fn closed_surfaces_code_and_reason_when_present() {
        assert_eq!(
            WsError::Closed(None).to_string(),
            "connection closed by peer"
        );
        let close = CloseFrame {
            code: 1006,
            reason: "abnormal".to_owned(),
        };
        assert_eq!(
            WsError::Closed(Some(close)).to_string(),
            "connection closed by peer (code 1006, reason \"abnormal\")"
        );
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WsError>();
    }
}
