//! The single concrete error type across the HTTP stack — transport and
//! middleware failures only.
//!
//! HTTP 4xx/5xx *statuses* are NOT errors here: they flow through as
//! `Ok(http::Response)` with the body intact for the adapter to classify
//! (ADR-0030 §5). Retry/CircuitBreaker peek `Response::status()` for their
//! resilience decisions.

use oath_adapter_net_api::{ErrorKind, HasErrorKind};

/// A boxed error source, preserving backend detail for logs without leaking the
/// concrete type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The single `Service::Error` (and every `Body::Error`) of the HTTP stack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpError {
    /// The request did not complete within its timeout.
    #[error("request timed out")]
    Timeout,
    /// A connection-level failure (DNS, TCP, TLS, backend transport).
    #[error("connection failure")]
    Connection(#[source] BoxError),
    /// A pacing wait exceeded `max_wait` — the request was not sent.
    #[error("throttled: pacing wait exceeded max_wait")]
    Throttled,
    /// Credential stamping or refresh failed.
    #[error("authorization failed: {0}")]
    Auth(String),
    /// A backend error that does not fit another variant.
    #[error("network error")]
    Other(#[source] BoxError),
    /// The circuit breaker is open — the request was rejected without being sent.
    #[error("circuit open: request rejected without being sent")]
    CircuitOpen,
    /// A response body exceeded the configured maximum buffered size.
    #[error("response body exceeded the configured maximum")]
    BodyTooLarge,
}

impl HttpError {
    /// Construct an [`HttpError::Auth`] from a message. `AuthSource` impls use this.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    /// Construct an [`HttpError::Connection`] from a source error.
    #[must_use]
    pub fn connection(source: impl Into<BoxError>) -> Self {
        Self::Connection(source.into())
    }

    /// Construct an [`HttpError::Other`] from a source error.
    #[must_use]
    pub fn other(source: impl Into<BoxError>) -> Self {
        Self::Other(source.into())
    }
}

impl HasErrorKind for HttpError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Timeout => ErrorKind::Timeout,
            Self::Connection(_) => ErrorKind::Connection,
            Self::Throttled => ErrorKind::Throttled,
            Self::Auth(_) => ErrorKind::Auth,
            Self::Other(_) => ErrorKind::Unknown,
            Self::CircuitOpen => ErrorKind::CircuitOpen,
            Self::BodyTooLarge => ErrorKind::BodyTooLarge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HttpError;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(HttpError::Timeout.kind(), ErrorKind::Timeout);
        assert_eq!(HttpError::connection("reset").kind(), ErrorKind::Connection);
        assert_eq!(HttpError::Throttled.kind(), ErrorKind::Throttled);
        assert_eq!(HttpError::auth("expired").kind(), ErrorKind::Auth);
        assert_eq!(HttpError::other("boom").kind(), ErrorKind::Unknown);
        assert_eq!(HttpError::CircuitOpen.kind(), ErrorKind::CircuitOpen);
        assert_eq!(HttpError::BodyTooLarge.kind(), ErrorKind::BodyTooLarge);
    }

    #[test]
    fn auth_carries_message() {
        assert_eq!(
            HttpError::auth("no token").to_string(),
            "authorization failed: no token"
        );
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HttpError>();
    }
}
