//! The hyper backend leaf: a pooled `hyper_util` client over a rustls HTTPS
//! connector.
//!
//! Implements [`Service`], so it is an [`HttpClient`](oath_adapter_net_http_api::HttpClient)
//! by blanket impl (ADR-0030 §6). Response bodies stream by default, or buffer
//! per the request's `BufferMode` (ADR-0030 §4).

use crate::error::{map_hyper_err, map_legacy_err};
use bytes::Bytes;
use http_body::Body;
use http_body_util::combinators::MapErr;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer as HyperPoolTimer};
use oath_adapter_net_http_api::{BufferMode, HttpError, LimitedBody, ResponseBody, Service};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// Shared in-flight-request tracker so [`HyperLeaf::shutdown`] can drain before the
/// pooled connections drop — no `RST` of an in-flight order submission. `Arc`-shared
/// across leaf clones, so a drain on any clone observes every clone's traffic.
#[derive(Debug, Default)]
struct InFlight {
    count: AtomicUsize,
    drained: Notify,
}

/// RAII in-flight marker: bumps the count on [`enter`](InFlightGuard::enter), and on
/// drop decrements it and wakes any [`shutdown`](HyperLeaf::shutdown) waiter once the
/// count reaches zero.
struct InFlightGuard(Arc<InFlight>);

impl InFlightGuard {
    fn enter(tracker: &Arc<InFlight>) -> Self {
        tracker.count.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(tracker))
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        // `fetch_sub` returns the *previous* value; `1` means this was the last
        // in-flight request, so wake any drain waiter.
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.drained.notify_waiters();
        }
    }
}

/// The leaf response body: hyper's `Incoming` with its `hyper::Error` normalized
/// to [`HttpError`] (ADR-0030 §6).
///
/// `map_hyper_err` is a named `fn` so the type is nameable in [`HyperLeaf`]'s
/// associated type.
pub type HyperBody = MapErr<Incoming, fn(hyper::Error) -> HttpError>;

/// TLS trust anchors for [`hyper_leaf`]'s HTTPS connector.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TlsTrust {
    /// Bundled Mozilla roots (`webpki-roots`) — public CAs. The default posture
    /// for a public venue endpoint.
    WebpkiRoots,
    /// Trust **exactly** these DER-encoded certificates and nothing else — e.g.
    /// IBKR's self-signed Client Portal gateway cert. Public CAs are not trusted
    /// in this mode. Invalid entries are skipped; a fully-empty trust set then
    /// fails every handshake as a `Connection` error.
    CustomRoots(Vec<CertificateDer<'static>>),
}

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
    /// TLS trust anchors: bundled public roots, or a pinned custom set (e.g. a
    /// self-signed venue gateway).
    pub tls_trust: TlsTrust,
    /// Allow plaintext `http://` connections. **Default `false`** (HTTPS-only) so a
    /// misconfigured base URL cannot silently exfiltrate credentials over
    /// cleartext; set `true` only for a known-plaintext local gateway.
    pub allow_http: bool,
    /// HTTP/2 keepalive PING interval — `None` disables it (hyper's default). Set
    /// it so idle multiplexed connections to a long-lived venue are not silently
    /// reaped, at the cost of a periodic PING.
    pub http2_keep_alive_interval: Option<Duration>,
    /// How long to wait for a keepalive PING ack before treating the connection as
    /// dead. Ignored when `http2_keep_alive_interval` is `None`.
    pub http2_keep_alive_timeout: Duration,
    /// Send keepalive PINGs even with no active requests. Ignored when
    /// `http2_keep_alive_interval` is `None`.
    pub http2_keep_alive_while_idle: bool,
    /// Maximum bytes to buffer for a `BufferMode::Buffer` response; `None` =
    /// unbounded. Rejects an oversized body with [`HttpError::BodyTooLarge`]
    /// (ADR-0034 Amendment #13) — memory-safety for a misbehaving venue.
    pub max_response_bytes: Option<usize>,
}

/// The hyper backend leaf. `Clone` (the pool **and** the in-flight tracker are
/// `Arc`-shared internally), so the whole assembled `stack()` is `Clone`.
#[derive(Clone)]
pub struct HyperLeaf {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    inflight: Arc<InFlight>,
    max_response_bytes: Option<usize>,
}

impl std::fmt::Debug for HyperLeaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperLeaf")
            .field("in_flight", &self.inflight.count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl HyperLeaf {
    /// Drain: wait until every in-flight request has completed (or been dropped), so
    /// pooled connections can be dropped without `RST`ing an in-flight order
    /// submission. Returns immediately when nothing is in flight.
    ///
    /// This does **not** reject new calls — a caller draining for shutdown should
    /// stop issuing requests first, then `await` this. Idempotent and safe to call
    /// from any leaf clone (the tracker is shared).
    pub async fn shutdown(&self) {
        loop {
            // Arm the wake BEFORE reading the count, so a decrement that races the
            // read cannot be lost: `notify_waiters` only wakes already-armed waiters,
            // and `Notified::enable` arms this one now.
            let mut notified = std::pin::pin!(self.inflight.drained.notified());
            notified.as_mut().enable();
            if self.inflight.count.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
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
        // Borrow the pooled client (the returned future borrows `&self` per the
        // `Service` contract — no per-call `Arc` clone needed) (L4). Mark the request
        // in-flight from invocation until the future completes or is dropped, so
        // `shutdown` can drain it.
        let guard = InFlightGuard::enter(&self.inflight);
        let max_response_bytes = self.max_response_bytes;
        async move {
            let _guard = guard;
            // ADR-0030 §4: absent extension ⇒ Stream. `BufferMode` is `Copy`.
            let mode = req
                .extensions()
                .get::<BufferMode>()
                .copied()
                .unwrap_or(BufferMode::Stream);
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, Full::new(body));
            let resp = self.client.request(req).await.map_err(map_legacy_err)?;
            let (parts, incoming) = resp.into_parts();
            let body = match mode {
                BufferMode::Stream => {
                    let mapper: fn(hyper::Error) -> HttpError = map_hyper_err;
                    ResponseBody::streaming(incoming.map_err(mapper))
                },
                BufferMode::Buffer => {
                    // Collect inside the retry boundary → full-body retry coverage.
                    let bytes = match max_response_bytes {
                        Some(cap) => {
                            let cap = cap as u64;
                            if incoming
                                .size_hint()
                                .upper()
                                .is_some_and(|upper| upper > cap)
                            {
                                return Err(HttpError::BodyTooLarge);
                            }
                            LimitedBody::new(incoming.map_err(map_hyper_err), cap)
                                .collect()
                                .await?
                                .to_bytes()
                        },
                        None => incoming.collect().await.map_err(map_hyper_err)?.to_bytes(),
                    };
                    ResponseBody::buffered(bytes)
                },
            };
            Ok(http::Response::from_parts(parts, body))
        }
    }
}

/// Construct the pooled HTTPS leaf.
///
/// An `HttpConnector` (connect timeout, nodelay) wrapped by a rustls
/// `HttpsConnector` (aws-lc-rs, [`ConnConfig::tls_trust`] anchors, ALPN
/// h2+http/1.1), driven by a pooled `legacy::Client` on a `TokioExecutor`.
/// HTTPS-only unless [`ConnConfig::allow_http`] is set; optional HTTP/2 keepalive.
#[must_use]
pub fn hyper_leaf(conn: ConnConfig) -> HyperLeaf {
    let max_response_bytes = conn.max_response_bytes;
    let mut http = HttpConnector::new();
    http.enforce_http(false); // let the HTTPS wrapper handle `https://`
    http.set_connect_timeout(Some(conn.connect_timeout));
    http.set_nodelay(true);

    // Trust anchors: bundled public roots, or a pinned custom set (the same rustls
    // `ClientConfig` shape the TLS test builds by hand).
    let tls = hyper_rustls::HttpsConnectorBuilder::new();
    let tls = match conn.tls_trust {
        TlsTrust::WebpkiRoots => tls.with_webpki_roots(),
        TlsTrust::CustomRoots(certs) => {
            let mut roots = RootCertStore::empty();
            let (_added, _skipped) = roots.add_parsable_certificates(certs);
            let client_cfg = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            tls.with_tls_config(client_cfg)
        },
    };
    // HTTPS-only by default; plaintext is opt-in so a stray `http://` cannot leak
    // credentials in cleartext.
    let tls = if conn.allow_http {
        tls.https_or_http()
    } else {
        tls.https_only()
    };
    let https = tls.enable_http1().enable_http2().wrap_connector(http);

    let client = Client::builder(TokioExecutor::new())
        .timer(HyperPoolTimer::new())
        .pool_idle_timeout(conn.pool_idle_timeout)
        .pool_max_idle_per_host(conn.pool_max_idle_per_host)
        .http2_keep_alive_interval(conn.http2_keep_alive_interval)
        .http2_keep_alive_timeout(conn.http2_keep_alive_timeout)
        .http2_keep_alive_while_idle(conn.http2_keep_alive_while_idle)
        .build(https);

    HyperLeaf {
        client,
        inflight: Arc::new(InFlight::default()),
        max_response_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnConfig, HyperLeaf, TlsTrust, hyper_leaf};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::BufferMode;
    use oath_adapter_net_http_api::Service;
    use std::convert::Infallible;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn test_conn() -> ConnConfig {
        ConnConfig {
            pool_max_idle_per_host: 4,
            pool_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(2),
            // The plain-HTTP echo/abort/truncate servers in these tests need it;
            // production defaults to HTTPS-only.
            tls_trust: TlsTrust::WebpkiRoots,
            allow_http: true,
            http2_keep_alive_interval: None,
            http2_keep_alive_timeout: Duration::from_secs(10),
            http2_keep_alive_while_idle: false,
            max_response_bytes: None,
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

    // Serves a body of `n` bytes with an explicit Content-Length (via Full<Bytes>).
    async fn spawn_body_server(n: usize) -> String {
        let payload = Bytes::from(vec![b'x'; n]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let payload = payload.clone();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |_r| {
                        let payload = payload.clone();
                        async move {
                            Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                                payload,
                            )))
                        }
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
    async fn buffer_rejects_an_oversized_content_length_upfront() {
        let base = spawn_body_server(64).await;
        let conn = ConnConfig {
            max_response_bytes: Some(16),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/big"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let err = leaf
            .call(req)
            .await
            .expect_err("64-byte body over a 16-byte cap");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::BodyTooLarge),
            "expected BodyTooLarge, got {err:?}"
        );
    }

    // Sends a chunked (no Content-Length) body of two `chunk`-sized pieces.
    async fn spawn_chunked_server(chunk: &'static [u8], count: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0_u8; 256];
            loop {
                let n = stream.read(&mut tmp).await.unwrap();
                assert!(n > 0, "peer closed before a full request head");
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            for _ in 0..count {
                stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .unwrap();
                stream.write_all(chunk).await.unwrap();
                stream.write_all(b"\r\n").await.unwrap();
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
            stream.flush().await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn buffer_caps_an_unsized_streaming_body_while_collecting() {
        // Two 8-byte chunks = 16 bytes, no Content-Length, over a 10-byte cap.
        let base = spawn_chunked_server(b"12345678", 2).await;
        let conn = ConnConfig {
            max_response_bytes: Some(10),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/chunked"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let err = leaf
            .call(req)
            .await
            .expect_err("16 chunked bytes over a 10-byte cap");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::BodyTooLarge),
            "expected BodyTooLarge from the streaming cap, got {err:?}"
        );
    }

    #[tokio::test]
    async fn buffer_under_cap_collects_normally() {
        let base = spawn_body_server(8).await;
        let conn = ConnConfig {
            max_response_bytes: Some(1024),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/small"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let resp = leaf.call(req).await.expect("8-byte body under a 1KiB cap");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 8);
    }

    #[tokio::test]
    async fn buffer_with_no_cap_is_unbounded() {
        let base = spawn_body_server(64).await;
        let conn = ConnConfig {
            max_response_bytes: None,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/big"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let resp = leaf.call(req).await.expect("no cap → unbounded");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 64);
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
    async fn buffer_mode_collects_the_body_into_a_buffered_arm() {
        let base = spawn_echo_server(b"pong").await;
        let leaf = hyper_leaf(test_conn());
        let mut req = http::Request::get(format!("{base}/ping"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);

        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            resp.body().is_buffered(),
            "BufferMode::Buffer must yield a Buffered arm, got a streaming body"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"pong"));
    }

    #[tokio::test]
    async fn default_and_explicit_stream_keep_a_streaming_body() {
        let base = spawn_echo_server(b"pong").await;
        let leaf = hyper_leaf(test_conn());

        // No BufferMode extension → default Stream (ADR-0030 §4).
        let req = http::Request::get(format!("{base}/a"))
            .body(Bytes::new())
            .unwrap();
        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            resp.body().is_streaming(),
            "absent BufferMode must stay streaming"
        );

        // Explicit BufferMode::Stream → same.
        let mut req = http::Request::get(format!("{base}/b"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Stream);
        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            resp.body().is_streaming(),
            "explicit Stream must stay streaming"
        );
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

        let err = leaf.call(req).await.expect_err("must time out connecting");
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

        // The connection is established then dropped mid-exchange (a stale/closed
        // pooled connection). It now maps to HttpError::Connection — retryable and
        // breaker-visible (H1) — instead of the invisible Other/Unknown it used to be.
        let err = leaf.call(req).await.expect_err("aborted connection");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Connection(_)),
            "expected Connection, got {err:?}"
        );
    }

    // A raw-TCP server (not hyper's `service_fn`) that speaks a valid response
    // head promising a body longer than what it actually sends, then closes the
    // connection mid-body. This makes the error surface while the body is being
    // *streamed*, after the response head has already arrived successfully — so
    // it exercises `map_hyper_err` (the streaming-body mapper), not
    // `map_legacy_err` (the send/response-head mapper).
    async fn spawn_truncating_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Drain the request head so the client's write side completes cleanly.
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 256];
            loop {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0, "peer closed before sending a full request head");
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            // Promise 100 bytes of body, deliver 7, then drop the connection.
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            drop(stream);
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn truncated_response_body_surfaces_as_connection() {
        let base = spawn_truncating_server().await;
        let leaf = hyper_leaf(test_conn());
        let req = http::Request::get(format!("{base}/"))
            .body(Bytes::new())
            .unwrap();

        // The head is complete and valid, so the send phase (`map_legacy_err`)
        // succeeds — proving the subsequent failure comes from the body stream.
        let resp = leaf.call(req).await.expect("response head must arrive");
        assert_eq!(resp.status(), http::StatusCode::OK);

        // Unlike the send-phase tests above, the collected `Ok` payload
        // (`Collected<Bytes>`) *is* `Debug`, so clippy requires `expect_err`
        // here instead of the `.err().expect()` workaround.
        let err = resp
            .into_body()
            .collect()
            .await
            .expect_err("truncated body must error");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Connection(_)),
            "expected Connection (incomplete message), got {err:?}"
        );
    }

    #[tokio::test]
    async fn buffered_truncated_body_errors_from_call() {
        let base = spawn_truncating_server().await;
        let leaf = hyper_leaf(test_conn());
        let mut req = http::Request::get(format!("{base}/"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);

        // Under `BufferMode::Buffer`, `call` collects the body itself
        // (`incoming.collect().await.map_err(map_hyper_err)?`), so the truncated
        // body must fail `call` directly — unlike the streaming-path test above,
        // where `call` succeeds and the error only appears when the caller later
        // collects the body. This is what lets Retry re-drive the whole request.
        let err = leaf
            .call(req)
            .await
            .expect_err("truncated body must error from call() under Buffer mode");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Connection(_)),
            "expected Connection (retryable → full Buffer-mode retry coverage), got {err:?}"
        );
    }

    // Full TLS path THROUGH the production `hyper_leaf`: rcgen self-signed cert →
    // rustls server on loopback → a leaf built by
    // `hyper_leaf(TlsTrust::CustomRoots(..))` trusting exactly that cert. Proves the
    // constructor can reach a self-signed gateway (the IBKR Client Portal case) —
    // exercising the real aws-lc-rs handshake + webpki verification in CI.
    #[tokio::test]
    async fn hyper_leaf_round_trips_over_tls_with_a_custom_root() {
        use hyper_util::rt::TokioIo;
        use std::sync::Arc;
        use tokio_rustls::TlsAcceptor;

        // 1. Self-signed cert for "localhost".
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key_der =
            rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der()).unwrap();

        // 2. rustls server config with that cert.
        let server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await.unwrap();
            let io = TokioIo::new(tls);
            let svc = hyper::service::service_fn(|_req| async {
                Ok::<_, std::convert::Infallible>(hyper::Response::new(http_body_util::Full::new(
                    Bytes::from_static(b"secure"),
                )))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await;
        });

        // 3. Build the leaf via the PRODUCTION constructor, trusting only our cert,
        //    HTTPS-only (as in production).
        let conn = ConnConfig {
            tls_trust: TlsTrust::CustomRoots(vec![cert_der]),
            allow_http: false,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);

        // 4. Round-trip over https://localhost:PORT (SNI/cert CN = localhost).
        let req = http::Request::get(format!("https://localhost:{port}/"))
            .body(Bytes::new())
            .unwrap();
        let resp = leaf
            .call(req)
            .await
            .expect("tls round-trip via hyper_leaf custom root");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"secure"));
    }

    #[tokio::test]
    async fn https_only_rejects_plaintext_http() {
        // Default posture (`allow_http = false`) must refuse an `http://` URL rather
        // than send the request (and any credentials) in cleartext.
        let conn = ConnConfig {
            allow_http: false,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let req = http::Request::get("http://127.0.0.1:9/x")
            .body(Bytes::new())
            .unwrap();
        assert!(
            leaf.call(req).await.is_err(),
            "an HTTPS-only leaf must reject a plaintext http:// URL"
        );
    }

    #[tokio::test]
    async fn shutdown_pends_until_in_flight_drains() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let leaf = hyper_leaf(test_conn());
        // Simulate one in-flight request — exactly what `call` does internally.
        let guard = super::InFlightGuard::enter(&leaf.inflight);
        let mut shutdown = std::pin::pin!(leaf.shutdown());
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            shutdown.as_mut().poll(&mut cx).is_pending(),
            "shutdown must pend while a request is in flight"
        );
        drop(guard); // the request finished → the drain completes
        shutdown.await;
    }

    #[tokio::test]
    async fn call_tracks_in_flight_and_shutdown_drains_after_completion() {
        use std::sync::atomic::Ordering;
        let base = spawn_echo_server(b"pong").await;
        let leaf = hyper_leaf(test_conn());
        let req = http::Request::get(format!("{base}/x"))
            .body(Bytes::new())
            .unwrap();
        // `call` marks the request in-flight synchronously, before the first poll.
        let fut = leaf.call(req);
        assert_eq!(
            leaf.inflight.count.load(Ordering::Relaxed),
            1,
            "one request in flight"
        );
        fut.await.expect("round-trip");
        assert_eq!(
            leaf.inflight.count.load(Ordering::Relaxed),
            0,
            "drained on completion"
        );
        leaf.shutdown().await; // nothing in flight → returns immediately
    }

    // HTTP/2 keepalive (plumbing case): with `http2_keep_alive_interval` set and
    // `while_idle = true`, an h2-over-TLS connection stays usable across a brief
    // idle gap. The server is h2-only (ALPN = `h2`, an `http2::Builder`), so a
    // request that succeeds necessarily spoke HTTP/2 — the `HTTP_2` version
    // assertion makes that explicit. This proves the keepalive knobs thread
    // through `hyper_leaf` and that two requests succeed with keepalive
    // configured. It does NOT isolate the PING's effect: `test_conn()`'s 30s
    // `pool_idle_timeout` already covers the 250ms idle gap on its own, so the
    // connection would plausibly survive even with keepalive disabled, and the
    // test does not verify the second request reuses the same pooled connection.
    // See the defer note below for the causation/reaping case this does not cover.
    //
    // Deferred (Tier-1 → tracking): the NEGATIVE h2-keepalive case — an idle h2
    // connection being REAPED when keepalive is disabled — is not observed here,
    // nor is isolating keepalive's causal effect from the pool idle timeout noted
    // above. Both depend on hyper's/OS idle-connection timing and are flake-prone
    // as unit tests; the keepalive config knobs and the positive two-requests-
    // succeed path are covered above.
    #[tokio::test]
    async fn h2_keepalive_connection_survives_an_idle_gap() {
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use std::sync::Arc;
        use tokio_rustls::TlsAcceptor;

        // 1. Self-signed cert + rustls server config, ALPN-advertising `h2` only.
        //    Cert/key construction mirrors `hyper_leaf_round_trips_over_tls_with_a_
        //    custom_root`; the only addition is `alpn_protocols`.
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key_der =
            rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der()).unwrap();
        let mut server_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        server_cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls = acceptor.accept(tcp).await.unwrap();
                    let io = TokioIo::new(tls);
                    let svc = hyper::service::service_fn(|_req| async {
                        Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                            Bytes::from_static(b"h2ok"),
                        )))
                    });
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });

        // 2. Leaf trusting only our cert, HTTPS-only, keepalive PINGs enabled even
        //    while idle.
        let conn = ConnConfig {
            tls_trust: TlsTrust::CustomRoots(vec![cert_der]),
            allow_http: false,
            http2_keep_alive_interval: Some(Duration::from_millis(50)),
            http2_keep_alive_timeout: Duration::from_secs(5),
            http2_keep_alive_while_idle: true,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);

        let url = format!("https://localhost:{port}/x");
        // First request establishes the h2 connection.
        let r1 = leaf
            .call(http::Request::get(&url).body(Bytes::new()).unwrap())
            .await
            .expect("first h2 request");
        assert_eq!(r1.status(), http::StatusCode::OK);
        assert_eq!(
            r1.version(),
            http::Version::HTTP_2,
            "server is h2-only, so a successful response must be HTTP/2"
        );
        let b1 = r1.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(b1, Bytes::from_static(b"h2ok"));

        // Idle for several keepalive intervals (50ms each) — well within
        // `test_conn()`'s 30s pool_idle_timeout, so this does not isolate the
        // PING's effect. A second request must still succeed with keepalive
        // configured.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let r2 = leaf
            .call(http::Request::get(&url).body(Bytes::new()).unwrap())
            .await
            .expect("second h2 request over a kept-alive connection");
        assert_eq!(r2.status(), http::StatusCode::OK);
        assert_eq!(r2.version(), http::Version::HTTP_2);
    }
}
