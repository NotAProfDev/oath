//! The `HttpClient` dependency-inversion seam for adapters.
//!
//! A backend implements [`Service`] once and is `HttpClient` for free via blanket
//! impl (ADR-0030 §6). Per ADR-0029 §5 this is a compile-time seam — never `dyn`.

use crate::{HttpError, Service};
use bytes::Bytes;
use std::future::Future;

/// A composed HTTP client: a [`Service`] from `http::Request<Bytes>` to
/// `http::Response<Self::Body>` with `Error = HttpError`.
///
/// # Example
/// The `HttpClient` seam is what adapters depend on — construct one with
/// [`stack()`](crate::stack()) or the hyper `build()` and send through this trait's
/// [`send`](HttpClient::send):
/// ```no_run
/// use oath_adapter_net_http_api::HttpClient;
/// use bytes::Bytes;
///
/// pub async fn fetch(client: &impl HttpClient, req: http::Request<Bytes>) {
///     let _ = client.send(req).await;
/// }
/// # fn main() {}
/// ```
pub trait HttpClient:
    Service<http::Request<Bytes>, Response = http::Response<Self::Body>, Error = HttpError>
{
    /// The response body type (generic, for zero-alloc flow-through).
    type Body: http_body::Body<Data = Bytes, Error = HttpError>;

    /// Send a request — sugar over [`Service::call`].
    fn send(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<http::Response<Self::Body>, HttpError>> + Send {
        self.call(req)
    }
}

impl<S, B> HttpClient for S
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>,
    B: http_body::Body<Data = Bytes, Error = HttpError>,
{
    type Body = B;
}

#[cfg(test)]
mod tests {
    use super::HttpClient;
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // Minimal body whose error is `HttpError` (stock `Full`/`Empty` are `Infallible`).
    struct EmptyBody;
    impl Body for EmptyBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(None)
        }
        fn is_end_stream(&self) -> bool {
            true
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(0)
        }
    }

    #[derive(Clone)]
    struct Leaf;
    impl Service<http::Request<Bytes>> for Leaf {
        type Response = http::Response<EmptyBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl std::future::Future<Output = Result<Self::Response, HttpError>> + Send {
            async { Ok(http::Response::new(EmptyBody)) }
        }
    }

    #[test]
    fn any_matching_service_is_httpclient() {
        fn assert_http_client<C: HttpClient>(_: &C) {}
        assert_http_client(&Leaf); // blanket impl applies
    }

    #[tokio::test]
    async fn send_is_sugar_over_call() {
        let resp = HttpClient::send(&Leaf, http::Request::new(Bytes::new()))
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
    }
}
