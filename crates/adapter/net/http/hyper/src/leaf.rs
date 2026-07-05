//! The hyper backend leaf: a pooled `hyper_util` client over a rustls HTTPS
//! connector.
//!
//! Implements [`Service`], so it is an [`HttpClient`](oath_adapter_net_http_api::HttpClient)
//! by blanket impl (ADR-0030 §6). Response bodies stream (PR A); buffering is PR B.

use crate::error::{map_hyper_err, map_legacy_err};
use bytes::Bytes;
use http_body_util::combinators::MapErr;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer as HyperPoolTimer};
use oath_adapter_net_http_api::{HttpError, ResponseBody, Service};
use std::future::Future;
use std::time::Duration;

/// The leaf response body: hyper's `Incoming` with its `hyper::Error` normalized
/// to [`HttpError`] (ADR-0030 §6).
///
/// `map_hyper_err` is a named `fn` so the type is nameable in [`HyperLeaf`]'s
/// associated type.
pub type HyperBody = MapErr<Incoming, fn(hyper::Error) -> HttpError>;

/// Connection-pool + connector configuration for [`hyper_leaf`]. Plain data — no
/// `serde`, no type parameter (like `HttpConfig`); adapters construct it directly.
#[derive(Debug, Clone)]
pub struct ConnConfig {
    /// Max idle pooled connections retained per host.
    pub pool_max_idle_per_host: usize,
    /// How long an idle pooled connection is retained before eviction.
    pub pool_idle_timeout: Duration,
    /// Bound on TCP connect + TLS handshake — fails fast on a dead host,
    /// independent of (and tighter than) the per-attempt `Timeout` layer.
    pub connect_timeout: Duration,
}

/// The hyper backend leaf. `Clone` (the pool is `Arc`-shared internally), so the
/// whole assembled `stack()` is `Clone`.
#[derive(Clone)]
pub struct HyperLeaf {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl std::fmt::Debug for HyperLeaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperLeaf").finish_non_exhaustive()
    }
}

impl Service<http::Request<Bytes>> for HyperLeaf {
    type Response = http::Response<ResponseBody<HyperBody>>;
    type Error = HttpError;

    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        let client = self.client.clone();
        async move {
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, Full::new(body));
            let resp = client.request(req).await.map_err(map_legacy_err)?;
            let (parts, incoming) = resp.into_parts();
            let mapper: fn(hyper::Error) -> HttpError = map_hyper_err;
            let body = ResponseBody::streaming(incoming.map_err(mapper));
            Ok(http::Response::from_parts(parts, body))
        }
    }
}

/// Construct the pooled HTTPS leaf.
///
/// An `HttpConnector` (connect timeout, nodelay) wrapped by a rustls
/// `HttpsConnector` (aws-lc-rs, webpki-roots, ALPN h2+http/1.1), driven by a
/// pooled `legacy::Client` on a `TokioExecutor`.
// `conn`'s fields are all `Copy` and only read here, but the public signature
// takes it by value to match `stack()`'s "config consumed once at boot" shape.
#[allow(clippy::needless_pass_by_value)]
#[must_use]
pub fn hyper_leaf(conn: ConnConfig) -> HyperLeaf {
    let mut http = HttpConnector::new();
    http.enforce_http(false); // let the HTTPS wrapper handle `https://`
    http.set_connect_timeout(Some(conn.connect_timeout));
    http.set_nodelay(true);

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(http);

    let client = Client::builder(TokioExecutor::new())
        .timer(HyperPoolTimer::new())
        .pool_idle_timeout(conn.pool_idle_timeout)
        .pool_max_idle_per_host(conn.pool_max_idle_per_host)
        .build(https);

    HyperLeaf { client }
}

#[cfg(test)]
mod tests {
    use super::{ConnConfig, HyperLeaf, hyper_leaf};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::Service;
    use std::convert::Infallible;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn test_conn() -> ConnConfig {
        ConnConfig {
            pool_max_idle_per_host: 4,
            pool_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(2),
        }
    }

    // Spawn a one-connection plain-HTTP hyper server that echoes a fixed body.
    // Returns the bound `http://127.0.0.1:PORT` base URL.
    async fn spawn_echo_server(reply: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |_req| async move {
                        Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                            Bytes::from_static(reply),
                        )))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn leaf_round_trips_a_plain_http_body() {
        let base = spawn_echo_server(b"pong").await;
        let leaf: HyperLeaf = hyper_leaf(test_conn());
        let req = http::Request::get(format!("{base}/ping"))
            .body(Bytes::new())
            .unwrap();

        let resp = leaf.call(req).await.expect("round-trip");
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"pong"));
    }

    #[tokio::test]
    async fn unreachable_host_hits_connect_timeout_as_connection_error() {
        // 203.0.113.0/24 (TEST-NET-3) is reserved and unroutable — connect stalls
        // until connect_timeout fires.
        let conn = ConnConfig {
            connect_timeout: Duration::from_millis(150),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let req = http::Request::get("http://203.0.113.1:9/x")
            .body(Bytes::new())
            .unwrap();

        // `Result::expect_err` needs `T: Debug`, but the `Ok` payload
        // (`Response<ResponseBody<HyperBody>>`) isn't `Debug` — go via `Option`
        // instead, which only needs `E: Debug` (satisfied by `HttpError`).
        let err = leaf
            .call(req)
            .await
            .err()
            .expect("must time out connecting");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Connection(_)),
            "expected Connection, got {err:?}"
        );
    }

    // A server that accepts then immediately drops the connection (no response).
    async fn spawn_aborting_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                drop(stream); // close without speaking HTTP
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn aborted_connection_surfaces_an_http_error() {
        let base = spawn_aborting_server().await;
        let leaf = hyper_leaf(test_conn());
        let req = http::Request::get(format!("{base}/x"))
            .body(Bytes::new())
            .unwrap();

        // The connection is established then dropped mid-exchange: hyper-util
        // reports it as a (non-connect) send error → HttpError::Other. We assert
        // it is an error and not a spurious success.
        let err = leaf.call(req).await.err().expect("aborted connection");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Other(_)),
            "expected Other, got {err:?}"
        );
    }
}
