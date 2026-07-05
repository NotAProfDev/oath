# net-http hyper backend — PR A (transport) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `oath-adapter-net-http-hyper` crate delivering `TokioTimer`, the pooled TLS hyper leaf (`hyper_leaf`/`ConnConfig`/`HyperLeaf`), the `hyper → HttpError` mapping, and `build()` (streaming responses), assembled through #88's `stack()`.

**Architecture:** A new backend crate — the first to own `hyper`/`tokio`/`rustls` (ADR-0030 §7). `HyperLeaf` implements `Service<http::Request<Bytes>>` over a `hyper_util` pooled `legacy::Client` on a rustls HTTPS connector; the blanket impl (ADR-0030 §6) makes it `HttpClient`. `build()` is a one-line delegation to `stack(hyper_leaf(conn), …)`. Response bodies always stream in PR A (`ResponseBody::streaming`); buffering is PR B.

**Tech Stack:** Rust 2024, `hyper` 1, `hyper-util` 0.1 (pooled `legacy::Client`, `TokioExecutor`), `hyper-rustls` 0.27 (aws-lc-rs + webpki-roots), `http-body-util` 0.1 (`Full`, `MapErr`), `tokio` 1. Tests: loopback `hyper` servers (plain + rustls), `rcgen` 0.13 self-signed certs.

**Spec:** [docs/superpowers/specs/2026-07-05-net-http-hyper-backend-design.md](../specs/2026-07-05-net-http-hyper-backend-design.md)

## Global Constraints

- **Edition 2024, MSRV 1.90.** Validate with `just msrv`.
- **No `unsafe`** (`unsafe_code = "deny"` workspace-wide).
- **No `unwrap`/`expect`/indexing in non-test code** (warned) — return `Result`, model errors with `thiserror`. Test code is exempt.
- **`missing_docs` warned** — every `pub` item gets a doc comment. Clippy `all` is **deny-level**; `pedantic`/`nursery` warn.
- **Definition of done = `just ci` passes** (fmt, lint, test, doc, deny, typos). Per net-http rule, run **`just doc`** in each task's checks — `check`/`lint`/`test` miss broken rustdoc intra-doc links.
- **Conventional Commits**, enforced by the `commit-msg` hook; **subject ≤ 72 chars**. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Worktree:** all work in `.claude/worktrees/net-http-hyper` on branch `feat/net-http-hyper` (already created off `main`; the spec commit is already there). Never touch the primary checkout's branch.
- **Dependency direction:** this crate depends on `oath-adapter-net-api` (Timer) and `oath-adapter-net-http-api` (everything else) — never the reverse. It is the sole owner of `hyper`/`tokio`/`rustls` production deps.
- **External-API note:** exact builder-method names in `hyper-util`/`hyper-rustls`/`rcgen`/`rustls` may vary by patch release. The code below is the known-good shape for the pinned majors; each TDD step ends with `just check`, which surfaces any drift immediately — resolve against `cargo doc -p <crate> --open` for the resolved version, keeping the documented behaviour identical.

---

### Task 1: Crate scaffold + workspace wiring + dependencies

Creates the empty crate, registers it as a workspace member, and pins the new external deps. Deliverable: the workspace compiles with an empty `oath-adapter-net-http-hyper`.

**Files:**
- Create: `crates/adapter/net/http/hyper/Cargo.toml`
- Create: `crates/adapter/net/http/hyper/src/lib.rs`
- Modify: `Cargo.toml` (root — `[workspace] members`, `[workspace.dependencies]`)

**Interfaces:**
- Consumes: nothing (scaffold).
- Produces: crate `oath-adapter-net-http-hyper`, importable as `oath_adapter_net_http_hyper`.

- [ ] **Step 1: Register the crate as a workspace member**

In root `Cargo.toml`, add to the `[workspace] members` list (keep it grouped with the other `crates/adapter/net/http/*` entries, alphabetical):

```toml
  "crates/adapter/net/http/hyper",
```

- [ ] **Step 2: Add the internal dep pin + external dep pins**

In root `Cargo.toml` `[workspace.dependencies]`, add the internal-crate pin next to the other `oath-adapter-net-http-*` lines:

```toml
oath-adapter-net-http-hyper = { path = "crates/adapter/net/http/hyper", version = "0.1.0" }
```

Then, in the same table, add the external pins (backend-specific deps live here so the whole workspace pins one version; `cargo-deny` sees them here first):

```toml
hyper = { version = "1", features = ["client", "http1", "http2"] }
hyper-util = { version = "0.1", features = ["client", "client-legacy", "http1", "http2", "tokio"] }
hyper-rustls = { version = "0.27", default-features = false, features = ["http1", "http2", "aws-lc-rs", "webpki-roots"] }
rustls = { version = "0.23", default-features = false, features = ["aws-lc-rs"] }
rcgen = "0.13"
```

- [ ] **Step 3: Write the crate manifest**

Create `crates/adapter/net/http/hyper/Cargo.toml`:

```toml
[package]
name = "oath-adapter-net-http-hyper"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
oath-adapter-net-api = { workspace = true }
oath-adapter-net-http-api = { workspace = true }
bytes = { workspace = true }
http = { workspace = true }
http-body-util = { workspace = true }
hyper = { workspace = true }
hyper-util = { workspace = true }
hyper-rustls = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
rustls = { workspace = true }
rcgen = { workspace = true }
tracing-subscriber = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 4: Write the crate root**

Create `crates/adapter/net/http/hyper/src/lib.rs`:

```rust
//! The hyper backend for the OATH HTTP stack: a pooled, TLS-terminating leaf and
//! the `build()` construction surface (ADR-0030 §7).
//!
//! This is the only crate that depends on `hyper`/`tokio`/`rustls`. [`build`]
//! assembles the canonical resilience stack (`oath_adapter_net_http_api::stack`)
//! over a fresh [`hyper_leaf`], so backend choice stays behind the `HttpClient`
//! seam (ADR-0030 §6).
```

- [ ] **Step 5: Verify the workspace compiles**

Run: `just check`
Expected: PASS — `oath-adapter-net-http-hyper` compiles (empty), no new warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/adapter/net/http/hyper/
git commit -m "feat(net): scaffold oath-adapter-net-http-hyper crate"
```

---

### Task 2: `TokioTimer` — the real `Timer`

The tokio-backed `oath_adapter_net_api::Timer` the resilience layers run on.

**Files:**
- Create: `crates/adapter/net/http/hyper/src/timer.rs`
- Modify: `crates/adapter/net/http/hyper/src/lib.rs`

**Interfaces:**
- Consumes: `oath_adapter_net_api::Timer` (`fn sleep(&self, Duration) -> impl Future<Output=()> + Send`; supertrait `Clone + Send + Sync`).
- Produces: `pub struct TokioTimer;` implementing `Timer`.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/hyper/src/timer.rs`:

```rust
//! [`TokioTimer`] — the tokio-backed [`Timer`] the resilience stack sleeps on.

use oath_adapter_net_api::Timer;
use std::future::Future;
use std::time::Duration;

/// The tokio-backed [`Timer`]: [`sleep`](Timer::sleep) is `tokio::time::sleep`.
///
/// Zero-sized and `Copy` — the resilience layers hold it by value and clone it
/// across attempts at no cost.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(dur)
    }
}

#[cfg(test)]
mod tests {
    use super::TokioTimer;
    use oath_adapter_net_api::Timer;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn sleep_elapses_the_requested_duration() {
        let start = tokio::time::Instant::now();
        TokioTimer.sleep(Duration::from_secs(5)).await;
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }
}
```

Add to `crates/adapter/net/http/hyper/src/lib.rs`:

```rust
pub mod timer;

pub use timer::TokioTimer;
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `just test`
Expected: PASS — `sleep_elapses_the_requested_duration` (paused tokio time advances by exactly 5s). (This task's impl and test are written together because the impl is a two-line trait forward; the test is the real gate.)

- [ ] **Step 3: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS — no warnings, rustdoc links resolve.

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/hyper/src/timer.rs crates/adapter/net/http/hyper/src/lib.rs
git commit -m "feat(net): TokioTimer — the tokio-backed Timer"
```

---

### Task 3: The hyper leaf — happy-path round-trip

`HyperBody`, `ConnConfig`, `HyperLeaf`, `hyper_leaf`, and the error mappers, proven by a plain-HTTP loopback round-trip.

**Files:**
- Create: `crates/adapter/net/http/hyper/src/error.rs` (the `hyper → HttpError` mappers)
- Create: `crates/adapter/net/http/hyper/src/leaf.rs`
- Modify: `crates/adapter/net/http/hyper/src/lib.rs`

**Interfaces:**
- Consumes: `oath_adapter_net_http_api::{HttpClient, HttpError, ResponseBody}`; `oath_adapter_net_http_api::Service`; `http`, `bytes::Bytes`, `http_body_util::{Full, combinators::MapErr, BodyExt}`; `hyper::body::Incoming`; `hyper_util::client::legacy::{Client, connect::HttpConnector}`; `hyper_util::rt::{TokioExecutor, TokioTimer as HyperPoolTimer}`; `hyper_rustls::HttpsConnector`.
- Produces:
  - `pub type HyperBody = MapErr<Incoming, fn(hyper::Error) -> HttpError>;`
  - `pub struct ConnConfig { pub pool_max_idle_per_host: usize, pub pool_idle_timeout: Duration, pub connect_timeout: Duration }`
  - `pub struct HyperLeaf` — `Service<http::Request<Bytes>, Response = http::Response<ResponseBody<HyperBody>>, Error = HttpError>` (⇒ `HttpClient` by blanket impl)
  - `pub fn hyper_leaf(conn: ConnConfig) -> HyperLeaf`
  - `pub(crate) fn map_legacy_err(e: hyper_util::client::legacy::Error) -> HttpError`
  - `pub(crate) fn map_hyper_err(e: hyper::Error) -> HttpError`

- [ ] **Step 1: Write the error mappers**

Create `crates/adapter/net/http/hyper/src/error.rs`:

```rust
//! Anti-corruption: normalize `hyper`/`hyper-util` errors to [`HttpError`]
//! (ADR-0030 §6). Connect-phase failures (DNS/TCP/TLS/handshake, incl.
//! connect-timeout) map to [`HttpError::Connection`]; everything else — protocol
//! errors, cancellation, and mid-stream body errors — maps to [`HttpError::Other`]
//! ("network error"). No `Timeout`: semantic timeout is the `Timeout` *layer*.

use oath_adapter_net_http_api::HttpError;

/// Map a `hyper_util` client send error to [`HttpError`].
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
pub(crate) fn map_hyper_err(e: hyper::Error) -> HttpError {
    HttpError::other(e)
}
```

- [ ] **Step 2: Write the failing round-trip test + the leaf**

Create `crates/adapter/net/http/hyper/src/leaf.rs`:

```rust
//! The hyper backend leaf: a pooled `hyper_util` client over a rustls HTTPS
//! connector. Implements [`Service`], so it is an [`HttpClient`] by blanket impl
//! (ADR-0030 §6). Response bodies stream (PR A); buffering is PR B.

use crate::error::{map_hyper_err, map_legacy_err};
use bytes::Bytes;
use http_body_util::combinators::MapErr;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioTimer as HyperPoolTimer};
use oath_adapter_net_api::Service;
use oath_adapter_net_http_api::{HttpError, ResponseBody};
use std::future::Future;
use std::time::Duration;

/// The leaf response body: hyper's `Incoming` with its `hyper::Error` normalized
/// to [`HttpError`] (ADR-0030 §6). `map_hyper_err` is a named `fn` so the type is
/// nameable in [`HyperLeaf`]'s associated type.
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

/// Construct the pooled HTTPS leaf: an `HttpConnector` (connect timeout, nodelay)
/// wrapped by a rustls `HttpsConnector` (aws-lc-rs, webpki-roots, ALPN
/// h2+http/1.1), driven by a pooled `legacy::Client` on a `TokioExecutor`.
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
    use super::{hyper_leaf, ConnConfig, HyperLeaf};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Service;
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
                        Ok::<_, Infallible>(hyper::Response::new(
                            http_body_util::Full::new(Bytes::from_static(reply)),
                        ))
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
}
```

Add to `crates/adapter/net/http/hyper/src/lib.rs`:

```rust
mod error;
pub mod leaf;

pub use leaf::{hyper_leaf, ConnConfig, HyperBody, HyperLeaf};
```

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `just check`
Expected: compiles (resolve any external builder-method drift per the Global Constraints note).
Run: `just test`
Expected: PASS — `leaf_round_trips_a_plain_http_body` returns `200 pong`.

- [ ] **Step 4: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/hyper/src/error.rs crates/adapter/net/http/hyper/src/leaf.rs crates/adapter/net/http/hyper/src/lib.rs
git commit -m "feat(net): hyper leaf — ConnConfig, HyperLeaf, hyper_leaf"
```

---

### Task 4: Leaf error paths — connect failure + connect timeout

Proves the error mapping: an aborted connection surfaces a non-`Connection` protocol error, and an unreachable host trips `connect_timeout` → `HttpError::Connection`.

**Files:**
- Modify: `crates/adapter/net/http/hyper/src/leaf.rs` (add tests only)

**Interfaces:**
- Consumes: `HyperLeaf`, `hyper_leaf`, `ConnConfig`, `HttpError` (from Task 3).
- Produces: nothing new (test-only).

- [ ] **Step 1: Write the connect-timeout failing test**

Add to the `tests` module in `crates/adapter/net/http/hyper/src/leaf.rs`:

```rust
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
```

- [ ] **Step 2: Write the server-abort failing test**

Add a server that drops the connection before replying, and a test asserting the send errors. Add this helper and test to the same `tests` module:

```rust
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
        let err = leaf.call(req).await.expect_err("aborted connection");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::Other(_)),
            "expected Other, got {err:?}"
        );
    }
```

- [ ] **Step 3: Run the tests**

Run: `just test`
Expected: PASS — both error-path tests classify correctly. (If `aborted_connection_surfaces_an_http_error` is flaky because the drop races the connect classification, tighten the server to complete the TCP handshake before dropping — it already does, since `accept()` returns a connected stream.)

- [ ] **Step 4: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/hyper/src/leaf.rs
git commit -m "test(net): hyper leaf error-path mapping (connect + abort)"
```

---

### Task 5: TLS loopback — real aws-lc-rs/webpki handshake in CI

Stands up a rustls loopback server with an `rcgen` self-signed cert and drives a `HyperLeaf` whose connector trusts that cert, exercising the full TLS path deterministically.

The test constructs `HyperLeaf { client }` directly with a custom-root client — so it must be an **in-crate** `#[cfg(test)]` test (private-field access), added to the existing `tests` module in `leaf.rs`. An external `tests/` integration file cannot see the private `client` field and would need an exported constructor seam we don't want in the public API; the in-crate test avoids both.

**Files:**
- Modify: `crates/adapter/net/http/hyper/src/leaf.rs` (add the TLS test to the existing in-crate `tests` module)
- Modify: `crates/adapter/net/http/hyper/Cargo.toml` + root `Cargo.toml` (add `tokio-rustls` dev-dep + pin)

**Interfaces:**
- Consumes: `HyperLeaf` and its private `client` field (Task 3); `rustls`, `rcgen`, `tokio-rustls`, `hyper`, `hyper-util`, `hyper-rustls`, `tokio` (dev-deps).
- Produces: nothing new (test-only; no public API change).

- [ ] **Step 1: Write the TLS round-trip failing test**

Add to the `tests` module in `crates/adapter/net/http/hyper/src/leaf.rs` (imports at the top of the module as needed):

```rust
    // Full TLS path: rcgen self-signed cert → rustls server on loopback → a
    // HyperLeaf whose connector trusts exactly that cert. Exercises the real
    // aws-lc-rs handshake + webpki verification in CI (custom root, not webpki-roots).
    #[tokio::test]
    async fn leaf_round_trips_over_tls_with_a_trusted_self_signed_cert() {
        use hyper_util::client::legacy::connect::HttpConnector;
        use hyper_util::client::legacy::Client;
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use std::sync::Arc;
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;

        // 1. Self-signed cert for "localhost".
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(
            cert.key_pair.serialize_der(),
        )
        .unwrap();

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
                Ok::<_, std::convert::Infallible>(hyper::Response::new(
                    http_body_util::Full::new(Bytes::from_static(b"secure")),
                ))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await;
        });

        // 3. Client root store trusting only our self-signed cert.
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let client_cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let mut http = HttpConnector::new();
        http.enforce_http(false);
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(client_cfg)
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        let client = Client::builder(TokioExecutor::new()).build(https);
        let leaf = HyperLeaf { client };

        // 4. Round-trip over https://localhost:PORT (SNI/cert CN = localhost).
        let req = http::Request::get(format!("https://localhost:{port}/"))
            .body(Bytes::new())
            .unwrap();
        let resp = leaf.call(req).await.expect("tls round-trip");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"secure"));
    }
```

Add `tokio-rustls` to dev-deps in `crates/adapter/net/http/hyper/Cargo.toml`:

```toml
tokio-rustls = { version = "0.26", default-features = false, features = ["aws-lc-rs"] }
```

and its workspace pin in root `Cargo.toml` `[workspace.dependencies]`:

```toml
tokio-rustls = { version = "0.26", default-features = false, features = ["aws-lc-rs"] }
```

- [ ] **Step 2: Run the test**

Run: `just test`
Expected: PASS — `leaf_round_trips_over_tls_with_a_trusted_self_signed_cert` returns `secure` over a real TLS handshake. (Resolve any rcgen/rustls method drift per the Global Constraints note; behaviour — trust exactly the self-signed cert, round-trip — stays identical.)

- [ ] **Step 3: Verify lint + docs + deny (first hyper/rustls deny contact)**

Run: `just lint && just doc`
Expected: PASS.
Run: `just deny`
Expected: PASS — if the hyper/rustls/aws-lc tree trips an advisory/license/ban, record the resolution (allowlist entry with justification) in `deny.toml` and re-run. This is a manifest decision, not a design change (spec Risks).

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/hyper/src/leaf.rs crates/adapter/net/http/hyper/Cargo.toml Cargo.toml
git commit -m "test(net): TLS loopback round-trip over self-signed cert"
```

---

### Task 6: `build()` — delegate to `stack()`

The construction surface: `build() = stack(hyper_leaf(conn), …)`, proven by a smoke round-trip through the full stack over the loopback and a `BuildError` on bad coverage.

**Files:**
- Create: `crates/adapter/net/http/hyper/src/build.rs`
- Modify: `crates/adapter/net/http/hyper/src/lib.rs`

**Interfaces:**
- Consumes: `hyper_leaf`, `ConnConfig` (Task 3); `oath_adapter_net_http_api::{stack, HttpConfig, HttpClient, AuthSource}`; `oath_adapter_net_http_api::rate::{RateKey, RateLimitConfig, BuildError}`; `oath_adapter_net_api::Timer`.
- Produces: `pub fn build<T, A, K>(cfg: HttpConfig, timer: T, auth: A, rate_limits: RateLimitConfig<K>, conn: ConnConfig) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>`.

- [ ] **Step 1: Write `build()`**

Create `crates/adapter/net/http/hyper/src/build.rs`:

```rust
//! [`build`] — the hyper construction surface. Assembles the canonical resilience
//! stack (ADR-0031 §1) over a fresh pooled hyper leaf by delegating to
//! `oath_adapter_net_http_api::stack`; ordering invariants stay tested there (#88).

use crate::leaf::{hyper_leaf, ConnConfig};
use oath_adapter_net_api::Timer;
use oath_adapter_net_http_api::rate::{BuildError, RateKey, RateLimitConfig};
use oath_adapter_net_http_api::{stack, AuthSource, HttpClient, HttpConfig};
use std::fmt;

/// Assemble the canonical resilience stack over a fresh pooled hyper leaf.
///
/// `build(cfg, timer, auth, rate_limits, conn) == stack(hyper_leaf(conn), cfg,
/// timer, auth, rate_limits)`. The return is fully opaque — adapters use it
/// through the `HttpClient` seam, never naming the concrete layered type.
///
/// # Errors
/// [`BuildError`] from `stack()` if `rate_limits` is not total over `K::all()`,
/// a policy is out of range, or the concurrency-singleton invariant is breached.
pub fn build<T, A, K>(
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
    conn: ConnConfig,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where
    T: Timer + 'static,
    A: AuthSource + 'static,
    K: RateKey + fmt::Debug,
{
    stack(hyper_leaf(conn), cfg, timer, auth, rate_limits)
}
```

Add to `crates/adapter/net/http/hyper/src/lib.rs`:

```rust
pub mod build;

pub use build::build;
```

Confirm the `stack`-side re-export path: `oath_adapter_net_http_api` re-exports `stack`, `HttpConfig`, `AuthSource`, `HttpClient` at the crate root, and `RateKey`/`RateLimitConfig`/`BuildError` from its `rate` module (also re-exported at root as `BuildError`/`RateKey`/`RateLimitConfig`). If the `rate::` path fails to resolve, import them from the crate root instead (`oath_adapter_net_http_api::{BuildError, RateKey, RateLimitConfig}`).

- [ ] **Step 2: Write the smoke + BuildError tests**

Add to the bottom of `crates/adapter/net/http/hyper/src/build.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::build;
    use crate::leaf::ConnConfig;
    use crate::timer::TokioTimer;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Service;
    use oath_adapter_net_http_api::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use oath_adapter_net_http_api::{
        BuildError, CircuitBreakerConfig, HttpConfig, NoAuth, RateKey, RateScope, RetryConfig,
        Scope,
    };
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::num::NonZeroU32;
    use std::time::Duration;
    use tokio::net::TcpListener;

    // A single-variant, drift-proof RateKey (compiler-checked `all()`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Rest,
    }
    impl RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Rest]
        }
    }

    fn conn() -> ConnConfig {
        ConnConfig {
            pool_max_idle_per_host: 4,
            pool_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(2),
        }
    }

    fn http_cfg() -> HttpConfig {
        HttpConfig {
            timeout: Duration::from_secs(5),
            retry: RetryConfig {
                max_attempts: NonZeroU32::new(1).unwrap(),
                base: Duration::ZERO,
                cap: Duration::ZERO,
                seed: 1,
            },
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: NonZeroU32::new(3).unwrap(),
                cooldown: Duration::from_secs(30),
                throttle_cooldown: Duration::from_secs(900),
                half_open_probes: NonZeroU32::new(1).unwrap(),
            },
            headers: http::HeaderMap::new(),
            rate_limit_max_wait: Duration::ZERO,
        }
    }

    // A total pacing config over Key (global unlimited; Rest global-only).
    fn total_rates() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::from([(Key::Rest, LimitDecl::GlobalOnly)]),
        }
    }

    async fn spawn_echo() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(|_r| async {
                        Ok::<_, Infallible>(hyper::Response::new(
                            http_body_util::Full::new(Bytes::from_static(b"ok")),
                        ))
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
    async fn build_assembles_a_working_stack_over_the_hyper_leaf() {
        let base = spawn_echo().await;
        let client = build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");

        let mut req = http::Request::get(format!("{base}/x"))
            .body(Bytes::new())
            .unwrap();
        // Fail-closed pacing: every request carries an explicit Scope (ADR-0034 #1).
        req.extensions_mut().insert(RateScope::<Key> {
            scope: Scope::Global,
            key: None,
        });

        let resp = client.call(req).await.expect("round-trip through the stack");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok"));
    }

    #[test]
    fn build_rejects_a_config_missing_pacing_coverage() {
        // `local` empty ⇒ Key::Rest unclassified ⇒ BuildError before any leaf work.
        let rates = RateLimitConfig::<Key> {
            global: LimitPolicy::TokenBucket {
                rate: 1000,
                per: Duration::from_secs(1),
                burst: 1000,
            },
            local: HashMap::new(),
        };
        let err = build(http_cfg(), TokioTimer, NoAuth, rates, conn())
            .expect_err("missing coverage must fail closed");
        assert!(
            matches!(err, BuildError::UndeclaredKey(_)),
            "expected UndeclaredKey, got {err:?}"
        );
    }
}
```

Note: confirm the `RateScope` field/type shape (`RateScope<K> { scope: Scope, key: Option<K> }`) and the `BuildError::UndeclaredKey` variant name against `oath_adapter_net_http_api::rate` — both are used verbatim in #88's `stack.rs` tests (`req()` helper stamps `RateScope { scope, key }`; the missing-key test matches `BuildError::UndeclaredKey`).

- [ ] **Step 3: Run the tests**

Run: `just test`
Expected: PASS — `build_assembles_a_working_stack_over_the_hyper_leaf` round-trips `ok` through all layers; `build_rejects_a_config_missing_pacing_coverage` returns `BuildError::UndeclaredKey`.

- [ ] **Step 4: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/hyper/src/build.rs crates/adapter/net/http/hyper/src/lib.rs
git commit -m "feat(net): build() — assemble the stack over the hyper leaf"
```

---

### Task 7: Docs — ADR-0030 §7 amendment + CHANGELOG + full CI

Records the resolved TLS/connector decisions and closes the PR out against the full gate.

**Files:**
- Modify: `docs/adr/0030-http-transport-contract-wire-bytes-streaming-composition.md` (§7 amendment note)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

**Interfaces:**
- Consumes/Produces: none (docs).

- [ ] **Step 1: Amend ADR-0030 §7**

At the end of ADR-0030 §7 (after the "The cost is more wiring…" paragraph), add an amendment note:

```markdown
> **Amendment (2026-07-05, hyper-backend slice #<PR-A>).** The leaf's resolved TLS
> wiring: crypto provider **aws-lc-rs** (the rustls 0.23 default; FIPS-capable);
> trust anchors **webpki-roots** (bundled Mozilla roots — reproducible,
> container-friendly, no OS trust-store dependency); ALPN offers `h2`+`http/1.1`.
> `ConnConfig` exposes three knobs — `pool_max_idle_per_host`, `pool_idle_timeout`,
> and `connect_timeout` (a distinct, tighter bound on connect+handshake, separate
> from the per-attempt `Timeout` layer). See
> [the hyper-backend design](../superpowers/specs/2026-07-05-net-http-hyper-backend-design.md).
```

(Replace `#<PR-A>` with the actual PR number once opened.)

- [ ] **Step 2: Add the CHANGELOG entry**

In `CHANGELOG.md` under `## [Unreleased]` → `### Added` (create the subsection if absent), add:

```markdown
- **net-http hyper backend (transport).** New `oath-adapter-net-http-hyper` crate:
  `TokioTimer` (the tokio `Timer`), the pooled TLS leaf (`hyper_leaf`/`ConnConfig`/
  `HyperLeaf`) over hyper-util + rustls (aws-lc-rs, webpki-roots), the
  `hyper → HttpError` mapping, and `build()` delegating to `stack()`. Response
  bodies stream; buffering follows in PR B. (#<PR-A>)
```

- [ ] **Step 3: Run the full CI gate**

Run: `just ci`
Expected: PASS — fmt, lint, test, doc, deny, typos all green (identical to GitHub Actions). Also run `just msrv` to confirm the hyper/rustls tree builds on MSRV 1.90.

Run: `just msrv`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add docs/adr/0030-http-transport-contract-wire-bytes-streaming-composition.md CHANGELOG.md
git commit -m "docs(net): record hyper leaf TLS decisions (ADR-0030 §7)"
```

- [ ] **Step 5: Open the PR**

```bash
git push -u origin feat/net-http-hyper
gh issue create --title "feat(net): hyper backend transport — hyper_leaf + build() + TokioTimer (Slice, PR A)" \
  --label enhancement --body "The hyper-backend slice PR A: create oath-adapter-net-http-hyper with TokioTimer, the pooled TLS leaf, hyper→HttpError mapping, and build() delegating to stack(). Design: docs/superpowers/specs/2026-07-05-net-http-hyper-backend-design.md"
gh pr create --title "feat(net): hyper backend transport (hyper_leaf + build + TokioTimer)" \
  --body "Closes #<ISSUE>. See docs/superpowers/plans/2026-07-05-net-http-hyper-backend-pr-a.md. PR B (buffering) follows off main."
```

(Fill `#<ISSUE>` from the created issue; update the ADR/CHANGELOG `#<PR-A>` placeholders after the PR number is known, in a follow-up commit if desired.)

---

## Notes for the executor

- **Task 5's file plan was corrected inline:** the TLS test is an **in-crate** `#[cfg(test)]` test in `leaf.rs` (so it can construct `HyperLeaf { client }` with private-field access + custom roots), not a separate `tests/` integration file. No `pub(crate)` seam is exported.
- **PR B (buffering) is out of scope here** — it branches off `main` after this merges: the leaf reads `req.extensions().get::<BufferMode>()`, and on `BufferMode::Buffer` collects `Incoming` into `Bytes` → `ResponseBody::buffered(bytes)`. Additive; no signature/type change.
- **External-API drift:** every code step ends with `just check`/`just test`; a builder-method rename in a hyper/rustls patch release surfaces there. Keep the documented behaviour identical when resolving.
