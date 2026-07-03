//! The credential-stamping seam: `AuthSource`, `NoAuth`, and the `Auth` and
//! `SetHeaders` layers (ADR-0034).

use crate::HttpError;
use bytes::Bytes;
use std::future::Future;

/// The credential seam the adapter implements (ADR-0034 §1).
///
/// The `Auth` layer calls it innermost — inside `Retry`, once per attempt,
/// against the final buffered request — so per-attempt re-signing (fresh HMAC
/// timestamp/nonce) and current-token stamping are correct by construction.
pub trait AuthSource: Clone + Send + Sync {
    /// Stamp current credentials onto an outgoing request, immediately before
    /// send. Mutates in place (no clone — `Retry` already owns a per-attempt
    /// request). A failure (e.g. token refresh failed) is an
    /// [`HttpError::Auth`].
    fn authorize(
        &self,
        req: &mut http::Request<Bytes>,
    ) -> impl Future<Output = Result<(), HttpError>> + Send;
}

/// The no-op [`AuthSource`]: nothing to stamp. IBKR's local Client Portal
/// gateway holds the session cookie, so `authorize` is a ready `Ok(())`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAuth;

impl AuthSource for NoAuth {
    fn authorize(
        &self,
        _req: &mut http::Request<Bytes>,
    ) -> impl Future<Output = Result<(), HttpError>> + Send {
        std::future::ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthSource, NoAuth};
    use bytes::Bytes;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    #[test]
    fn no_auth_is_ready_ok_and_leaves_request_untouched() {
        let mut req = http::Request::new(Bytes::new());
        {
            let fut = NoAuth.authorize(&mut req);
            let mut cx = Context::from_waker(Waker::noop());
            let mut fut = pin!(fut);
            // Immediately ready on first poll — no executor, no runtime.
            assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
        }
        assert!(req.headers().is_empty());
    }

    #[test]
    fn auth_source_futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut req = http::Request::new(Bytes::new());
        let fut = NoAuth.authorize(&mut req);
        assert_send(&fut);
    }
}
