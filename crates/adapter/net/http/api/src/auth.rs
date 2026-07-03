//! The credential-stamping seam: `AuthSource`, `NoAuth`, and the `Auth` and
//! `SetHeaders` layers (ADR-0034).

use crate::{HttpError, Service};
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

/// The credential-stamping layer.
///
/// Runs [`AuthSource::authorize`] on the final request immediately before the
/// inner service (ADR-0034 §1). Sits innermost in the stack — inside `Retry` —
/// so credentials are re-stamped per attempt.
#[derive(Debug, Clone)]
pub struct Auth<S, A> {
    inner: S,
    auth: A,
}

impl<S, A> Auth<S, A> {
    /// Wrap `inner`, stamping credentials from `auth` before every call.
    #[must_use]
    pub const fn new(inner: S, auth: A) -> Self {
        Self { inner, auth }
    }
}

impl<S, A> Service<http::Request<Bytes>> for Auth<S, A>
where
    S: Service<http::Request<Bytes>, Error = HttpError> + Sync,
    A: AuthSource,
{
    type Response = S::Response;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`,
    // which only the desugared form can promise (ADR-0029 §4).
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        mut req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<S::Response, HttpError>> + Send {
        async move {
            self.auth.authorize(&mut req).await?;
            self.inner.call(req).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Auth, AuthSource, NoAuth};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
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

    /// Records every request it receives; the assertion surface for layer tests.
    #[derive(Clone, Default)]
    struct Recording {
        seen: Arc<Mutex<Vec<http::Request<Bytes>>>>,
    }

    impl Recording {
        fn seen(&self) -> Vec<http::Request<Bytes>> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Service<http::Request<Bytes>> for Recording {
        type Response = ();
        type Error = HttpError;

        fn call(
            &self,
            req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let seen = Arc::clone(&self.seen);
            async move {
                seen.lock().unwrap().push(req);
                Ok(())
            }
        }
    }

    /// Stamps a monotonically fresh `x-attempt` value on every authorize call.
    #[derive(Clone, Default)]
    struct Counting {
        n: Arc<AtomicU32>,
    }

    impl AuthSource for Counting {
        fn authorize(
            &self,
            req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
            req.headers_mut()
                .insert("x-attempt", http::HeaderValue::from(n));
            std::future::ready(Ok(()))
        }
    }

    /// Always fails — exercises the error short-circuit.
    #[derive(Clone)]
    struct Failing;

    impl AuthSource for Failing {
        fn authorize(
            &self,
            _req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            std::future::ready(Err(HttpError::auth("refresh failed")))
        }
    }

    #[tokio::test]
    async fn auth_stamps_the_request_the_inner_service_sees() {
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Counting::default());
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        let seen = leaf.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].headers()["x-attempt"], http::HeaderValue::from(1));
    }

    #[tokio::test]
    async fn auth_stamps_a_fresh_value_per_call() {
        // Each `call` is one attempt today; when `Retry` lands (Slice 1) it
        // re-invokes `call` per attempt, so this freshness IS the per-attempt
        // re-signing guarantee (ADR-0034 §1).
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Counting::default());
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        let seen = leaf.seen();
        assert_eq!(seen[0].headers()["x-attempt"], http::HeaderValue::from(1));
        assert_eq!(seen[1].headers()["x-attempt"], http::HeaderValue::from(2));
    }

    #[tokio::test]
    async fn authorize_error_short_circuits_and_classifies_as_auth() {
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Failing);
        let err = client
            .call(http::Request::new(Bytes::new()))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
        assert!(leaf.seen().is_empty(), "inner service must never be called");
    }

    #[test]
    fn auth_call_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let client = Auth::new(Recording::default(), NoAuth);
        let fut = client.call(http::Request::new(Bytes::new()));
        assert_send(&fut);
    }
}
