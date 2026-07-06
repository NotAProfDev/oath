//! Anti-corruption: normalize `hyper`/`hyper-util` errors to [`HttpError`]
//! (ADR-0030 §6). Connect-phase failures (DNS/TCP/TLS/handshake, incl.
//! connect-timeout) **and post-connect transport drops** (a reset/closed pooled
//! connection, an incomplete-message truncation, a cancelled/aborted transfer) map
//! to [`HttpError::Connection`] — retryable and breaker-visible (H1/H2). Genuine
//! protocol/other failures map to [`HttpError::Other`]. No `Timeout`: semantic
//! timeout is the `Timeout` *layer*.

use oath_adapter_net_http_api::HttpError;

/// Walk an error's source chain for a signal that the failure is a **transient
/// transport** condition: a `hyper::Error` reporting a dropped / incomplete /
/// closed / cancelled connection, or an `io::Error` of a connection-reset class.
///
/// This is the classic stale-pooled-connection case that `Retry` (transient-only)
/// and the `CircuitBreaker` exist for; collapsing it to `Other`/`Unknown` would
/// make it invisible to both (H1), and would make `BufferMode::Buffer`'s
/// mid-collect failures non-retryable, defeating its "full retry coverage" (H2).
fn is_transient_transport(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(h) = e.downcast_ref::<hyper::Error>()
            && (h.is_incomplete_message()
                || h.is_canceled()
                || h.is_body_write_aborted()
                || h.is_closed())
        {
            return true;
        }
        if let Some(io) = e.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
            )
        {
            return true;
        }
        cur = e.source();
    }
    false
}

/// Map a `hyper_util` client send error to [`HttpError`].
// `clippy::redundant_pub_crate` wants `pub` here since `error` is a private
// module — but that would trip `unreachable_pub` (also warn-level workspace-
// wide) instead, which correctly flags a `pub` item that is not actually
// reachable outside the crate. `pub(crate)` states the true, intended
// visibility; silence the (equally valid, but losing) alternative lint.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn map_legacy_err(e: hyper_util::client::legacy::Error) -> HttpError {
    // Connect-phase (DNS/TCP/TLS) *and* post-connect transport drops (a stale
    // pooled connection reset, an incomplete message, an H2 stream close) are all
    // `Connection` — the transients `Retry`/`CircuitBreaker` act on.
    if e.is_connect() || is_transient_transport(&e) {
        HttpError::connection(e)
    } else {
        HttpError::other(e)
    }
}

/// Map a `hyper` body/protocol error to [`HttpError`]. A truncated / incomplete /
/// reset body mid-transfer is a `Connection` failure (retryable, breaker-visible);
/// a genuine protocol error is `Other`.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn map_hyper_err(e: hyper::Error) -> HttpError {
    if is_transient_transport(&e) {
        HttpError::connection(e)
    } else {
        HttpError::other(e)
    }
}
