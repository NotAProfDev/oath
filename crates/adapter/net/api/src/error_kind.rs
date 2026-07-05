//! Error classification shared across the entire network stack.
//!
//! Backends map their concrete error types to [`ErrorKind`] via
//! [`HasErrorKind`]. Adapters branch only on [`ErrorKind`] — they never
//! pattern-match backend-specific error variants.

/// Coarse-grained classification of a network or protocol error.
///
/// Every backend maps its concrete error type to one of these variants via
/// [`HasErrorKind`]. Layers such as `RetryLayer` and `CircuitBreakerLayer`
/// branch on this value; they never inspect the underlying error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The operation did not complete within the allowed time.
    Timeout,

    /// A network-level failure: TCP reset, DNS failure, TLS handshake error, etc.
    Connection,

    /// The server signalled that the client is sending too many requests.
    Throttled,

    /// Credentials were missing, invalid, or expired.
    Auth,

    /// The request was well-formed but the server rejected it (4xx other than
    /// auth/throttle).
    Client,

    /// The server encountered an internal error (5xx).
    Server,

    /// The error does not fit any other category.
    Unknown,

    /// A circuit breaker rejected the request without sending it — the breaker is
    /// Open after prior failures (or a throttle) and is failing fast until its
    /// cooldown elapses. A deliberate local decision, not a transport outcome;
    /// non-retryable.
    CircuitOpen,
}

/// Implemented by error types that can be classified as an [`ErrorKind`].
///
/// Backends implement this on their concrete error types. Middleware layers
/// call [`HasErrorKind::kind`] to decide whether to retry, open a circuit, etc.
pub trait HasErrorKind {
    /// Return the coarse classification of this error.
    fn kind(&self) -> ErrorKind;
}
