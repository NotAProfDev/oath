//! A worked example of the mandatory per-request extension protocol for the net-http
//! stack: every request MUST carry an explicit `RateScope` (fail-closed — an absent
//! scope is rejected as `Throttled`, never sent); `Retryable` opts a request into the
//! Retry layer; `BufferMode` chooses streaming vs buffered response bodies.
//!
//! Run with: `cargo run -p oath-adapter-net-http-hyper --example client_with_directives`

// Examples are not library code: unwrap/expect on a local loopback round-trip keep
// the walkthrough readable, unlike the `Result`-returning style the library enforces.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytes::Bytes;
use http_body_util::BodyExt;
use oath_adapter_net_http_api::{
    BufferMode, CircuitBreakerConfig, HttpConfig, LimitDecl, LimitPolicy, NoAuth, RateKey,
    RateLimitConfig, RateScope, RetryConfig, Retryable, Service,
};
use oath_adapter_net_http_hyper::{ConnConfig, TlsTrust, TokioTimer, build};
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;
use std::time::Duration;
use tokio::net::TcpListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Endpoint {
    Rest,
}
impl RateKey for Endpoint {
    fn all() -> &'static [Self] {
        &[Self::Rest]
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // A local plaintext echo server stands in for the venue.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = hyper_util::rt::TokioIo::new(stream);
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(|_r| async {
                    Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                        Bytes::from_static(b"pong"),
                    )))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await;
            });
        }
    });

    let cfg = HttpConfig {
        timeout: Duration::from_secs(5),
        retry: RetryConfig {
            max_attempts: NonZeroU32::new(3).unwrap(),
            base: Duration::from_millis(50),
            cap: Duration::from_secs(1),
            seed: 1,
        },
        circuit_breaker: CircuitBreakerConfig {
            failure_rate_threshold: 50,
            window_size: NonZeroU32::new(50).unwrap(),
            minimum_calls: NonZeroU32::new(10).unwrap(),
            cooldown: Duration::from_secs(30),
            retry_after_fallback: Duration::from_secs(900),
            retry_after_cap: Duration::from_secs(1800),
            half_open_probes: NonZeroU32::new(1).unwrap(),
        },
        headers: http::HeaderMap::new(),
        rate_limit_max_wait: Duration::from_secs(0),
        body_stall_timeout: Some(Duration::from_secs(30)),
    };
    let rates = RateLimitConfig {
        global: LimitPolicy::TokenBucket {
            rate: 1000,
            per: Duration::from_secs(1),
            burst: 1000,
        },
        local: HashMap::from([(Endpoint::Rest, LimitDecl::GlobalOnly)]),
    };
    let conn = ConnConfig {
        pool_max_idle_per_host: 4,
        pool_idle_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(2),
        tls_trust: TlsTrust::WebpkiRoots,
        allow_http: true, // local plaintext gateway
        http2_keep_alive_interval: None,
        http2_keep_alive_timeout: Duration::from_secs(10),
        http2_keep_alive_while_idle: false,
    };

    let client = build(cfg, TokioTimer, NoAuth, rates, conn).expect("valid config");

    // ---- the per-request extension protocol ----
    let mut req = http::Request::get(format!("http://{addr}/quotes"))
        .body(Bytes::new())
        .unwrap();
    // MANDATORY: an explicit pacing scope. Omit it and the stack fails closed (Throttled).
    req.extensions_mut().insert(RateScope::<Endpoint>::Global);
    // OPTIONAL: opt into retries for this (idempotent) request.
    req.extensions_mut().insert(Retryable);
    // OPTIONAL: buffer the whole body inside the retry boundary (default is Stream).
    req.extensions_mut().insert(BufferMode::Buffer);

    let resp = client.call(req).await.expect("round-trip");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    println!("venue said: {}", String::from_utf8_lossy(&body));
    assert_eq!(body, Bytes::from_static(b"pong"));
}
