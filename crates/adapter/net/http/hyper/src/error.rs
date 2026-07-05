//! Anti-corruption: normalize `hyper`/`hyper-util` errors to [`HttpError`]
//! (ADR-0030 §6). Connect-phase failures (DNS/TCP/TLS/handshake, incl.
//! connect-timeout) map to [`HttpError::Connection`]; everything else — protocol
//! errors, cancellation, and mid-stream body errors — maps to [`HttpError::Other`]
//! ("network error"). No `Timeout`: semantic timeout is the `Timeout` *layer*.

use oath_adapter_net_http_api::HttpError;

/// Map a `hyper_util` client send error to [`HttpError`].
// `clippy::redundant_pub_crate` wants `pub` here since `error` is a private
// module — but that would trip `unreachable_pub` (also warn-level workspace-
// wide) instead, which correctly flags a `pub` item that is not actually
// reachable outside the crate. `pub(crate)` states the true, intended
// visibility; silence the (equally valid, but losing) alternative lint.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn map_legacy_err(e: hyper_util::client::legacy::Error) -> HttpError {
    if e.is_connect() {
        HttpError::connection(e)
    } else {
        HttpError::other(e)
    }
}

/// Map a `hyper` body/protocol error to [`HttpError`]. Body errors surface after
/// the response head, so there is no connect phase to distinguish — always
/// [`HttpError::Other`].
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn map_hyper_err(e: hyper::Error) -> HttpError {
    HttpError::other(e)
}
