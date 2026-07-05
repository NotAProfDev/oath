# net-http hyper backend — `hyper_leaf` + `build()` + `TokioTimer` — design

**Status:** Approved design, pre-implementation.
**Date:** 2026-07-05.
**Crate:** `oath-adapter-net-http-hyper` (`crates/adapter/net/http/hyper`) — **new**.
**Slice:** hyper-backend slice. Delivered as **two PRs** (PR A: transport; PR B:
buffering) — see [Delivery](#delivery-two-prs).

## Context

The [`stack()` assembly slice](2026-07-05-net-http-stack-assembly-design.md) (#88)
composes the canonical resilience stack — `Tracing → CircuitBreaker → Retry →
RateLimit → Timeout → SetHeaders → Auth → leaf` (ADR-0031 §1) — over an **arbitrary
`HttpClient` leaf** and returns `Result<impl HttpClient + Clone + Send + Sync +
'static, BuildError>`. Everything above the leaf, plus `TokioTimer`, was the last
runtime-free half; this slice supplies the runtime/TLS half it deferred:

| Deferred by #88 | Delivered here |
| --- | --- |
| `build()`, `hyper_leaf(conn)`, `ConnConfig` | `build() = stack(hyper_leaf(conn), …)` over the #88 assembly |
| `TokioTimer`, rustls/HTTPS connector, `hyper::Error → HttpError` | the real `Timer` + the pooled HTTPS leaf + anti-corruption error map |
| `BufferOrStream` (leaf-side buffered-xor-streaming) | PR B — the `Buffered` arm on the leaf |

This is the **first crate to own `hyper`/`tokio`/`rustls`** — ADR-0030 §7's
containment boundary. It adds no runtime dep to any existing crate; the leaf sits
behind the `HttpClient` seam (ADR-0030 §6), so a future `net-http-reqwest` is zero
churn to the stack.

### Governing decisions (inherited, not re-litigated)

- **Leaf = hyper-util pooled client over rustls, not reqwest** — [ADR-0030 §7].
- **`HttpClient` is a blanket-impl'd `Service` sub-trait** — [ADR-0030 §6]: a backend
  implements `Service` once and is `HttpClient` for free **once the body error is
  normalized to `HttpError`**.
- **`build()`/`stack()` split + return bound** — construction-surface spec, Seam #3;
  `build()` is a one-line delegation so the ordering invariants stay tested in #88.
- **`ResponseBody<B>` (buffered *xor* streaming), `BufferMode`, `Guarded<B>`** already
  ship in `net-http-api`. `Guarded` (the concurrency permit) is attached **by the
  `RateLimit` layer** (`Response = http::Response<Guarded<B>>`), *not* the leaf.

### Resolved decisions (this slice)

Four choices were open within §7's fixed frame; all resolved here:

- **Crypto provider — `aws-lc-rs`** (the rustls 0.23 default; FIPS-capable).
- **Trust anchors — `webpki-roots`** (bundled Mozilla roots; reproducible,
  container-friendly, no OS trust-store dependency).
- **`ConnConfig` — three knobs**: `pool_max_idle_per_host`, `pool_idle_timeout`,
  `connect_timeout`. ALPN auto-negotiates `h2`+`http/1.1`; `TCP_NODELAY` on.
- **Leaf testing — loopback + TLS**: plain-HTTP loopback for round-trip/error paths,
  plus a self-signed (`rcgen`) rustls loopback exercising the real aws-lc-rs/webpki
  handshake in CI.

## Goal

Deliver `TokioTimer`, `hyper_leaf(conn)`/`ConnConfig`, and `build()` so a real,
pooled, TLS-terminating HTTP client can be assembled through #88's `stack()` — with
the leaf's socket round-trip, body streaming, and `hyper::Error → HttpError` mapping
regression-tested deterministically over a loopback server (plain-HTTP + self-signed
TLS), no external network.

## Scope (in)

- **New crate** `oath-adapter-net-http-hyper` + workspace-member/dep wiring.
- **`TokioTimer`** — the real `oath_adapter_net_api::Timer` impl.
- **`HyperLeaf`** — `Service<http::Request<Bytes>>` over a `hyper_util` `legacy::Client`
  on a rustls HTTPS connector; blanket-impls `HttpClient`.
- **`ConnConfig`**, **`hyper_leaf(conn) -> HyperLeaf`**.
- **`build<T, A, K>(…) -> Result<impl HttpClient + …, BuildError>`** = `stack(hyper_leaf
  (conn), …)`.
- **`map_hyper_err`** — the `hyper`/`legacy::Error → HttpError` anti-corruption map.
- **PR B**: `BufferMode`-driven `Buffered` arm on the leaf.
- Loopback tests (plain-HTTP + TLS), ADR-0030 §7 amendment, CHANGELOG.

## Non-goals (deferred)

| Deferred item | Why | Lands with |
| --- | --- | --- |
| `serde` on `HttpConfig`/`ConnConfig` | Config deserialisation is an adapter concern (ADR-0003) | IBKR adapter slice |
| HTTP-version override, TCP keepalive, proxy | YAGNI — a future field is a deliberate reviewed breaking change | when a caller needs one |
| `rustls-native-certs` / OS trust store | webpki-roots chosen for reproducibility | not planned |
| A concrete `RateKey`/endpoint set | Backend-agnostic; the leaf is scheme/host-agnostic | IBKR adapter slice |

## Decisions

### `TokioTimer` — the real `Timer`

```rust
/// The tokio-backed [`Timer`]: `sleep` is `tokio::time::sleep`. `Clone + Send + Sync`,
/// zero-sized — the resilience layers hold it by value across attempts.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(dur)
    }
}
```

> **Name collision, deliberately managed.** `hyper_util::rt::TokioTimer` implements
> *hyper's* `hyper::rt::Timer` (used only for the connection pool's internal idle
> timers) and is a **different** type from this `TokioTimer`, which implements *oath's*
> `Timer` and feeds the resilience layers. hyper-util's is imported under an alias
> (`use hyper_util::rt::TokioTimer as HyperPoolTimer;`) at the one wiring site.

### `HyperBody` — normalize `Incoming`'s error to `HttpError`

ADR-0030 §6 requires the leaf body's `Error = HttpError`. `Incoming`'s native error is
`hyper::Error`, so the leaf wraps it with http-body-util's `MapErr` combinator — no
hand-rolled, pin-projected body, no `unsafe`:

```rust
/// The leaf response body: hyper's `Incoming` with its `hyper::Error` normalized to
/// `HttpError` (ADR-0030 §6). `map_hyper_err` is a named `fn` so the type is nameable.
pub type HyperBody = MapErr<Incoming, fn(hyper::Error) -> HttpError>;
```

### `HyperLeaf` — the `Service` leaf

```rust
/// The hyper backend leaf: a pooled hyper-util client over a rustls HTTPS connector.
/// `Clone` (the pool is `Arc`-shared) so the whole `stack()` is `Clone`.
#[derive(Clone)]
pub struct HyperLeaf {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl Service<http::Request<Bytes>> for HyperLeaf {
    type Response = http::Response<ResponseBody<HyperBody>>;
    type Error = HttpError;

    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        let client = self.client.clone();
        async move {
            // Bytes → Full<Bytes> (a one-frame request body).
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, Full::new(body));
            // Drive the pooled client; normalize the send error.
            let resp = client.request(req).await.map_err(map_legacy_err)?;
            // Wrap the Incoming body: normalize its error, then Streaming (PR A) —
            // PR B adds the BufferMode::Buffer branch here.
            let (parts, incoming) = resp.into_parts();
            let body = ResponseBody::streaming(incoming.map_err(map_hyper_err as fn(_) -> _));
            Ok(http::Response::from_parts(parts, body))
        }
    }
}
```

`type Response = http::Response<ResponseBody<HyperBody>>` **already in PR A** (always
the `Streaming` arm) — so `RateLimit` wraps it into the canonical
`Guarded<ResponseBody<HyperBody>>` and PR B adds only the `Buffered` arm, changing no
types. The blanket impl (ADR-0030 §6) makes `HyperLeaf: HttpClient` for free.

### `ConnConfig` + `hyper_leaf`

```rust
/// Connection-pool + connector configuration for the hyper leaf. Plain data (no
/// `serde`, no type parameter) — like `HttpConfig`, adapters construct it directly.
#[derive(Debug, Clone)]
pub struct ConnConfig {
    /// Max idle pooled connections retained per host.
    pub pool_max_idle_per_host: usize,
    /// How long an idle pooled connection is retained before eviction.
    pub pool_idle_timeout: Duration,
    /// Bound on TCP connect + TLS handshake. Fires fast on a dead host, independent of
    /// (and tighter than) the per-attempt `Timeout` layer, which bounds the whole send.
    pub connect_timeout: Duration,
}

/// Construct the pooled HTTPS leaf: `HttpConnector` (connect timeout, nodelay) → rustls
/// `HttpsConnector` (aws-lc-rs provider, webpki-roots, ALPN h2+http/1.1) → pooled
/// `legacy::Client` on a `TokioExecutor`.
pub fn hyper_leaf(conn: ConnConfig) -> HyperLeaf { /* … */ }
```

Connector wiring: `HttpConnector::new()`, `enforce_http(false)` (so the HTTPS wrapper
handles `https://`), `set_connect_timeout(Some(conn.connect_timeout))`,
`set_nodelay(true)`; wrapped by `hyper_rustls::HttpsConnectorBuilder` with a rustls
`ClientConfig` (aws-lc-rs `default_provider`, `webpki_roots::TLS_SERVER_ROOTS`),
`.https_or_http()`, ALPN `h2`+`http/1.1`. Client:
`Client::builder(TokioExecutor::new()).timer(HyperPoolTimer::new()).pool_idle_timeout
(conn.pool_idle_timeout).pool_max_idle_per_host(conn.pool_max_idle_per_host).build
(connector)`.

### Error mapping — anti-corruption

The send `Result` and the body carry **distinct** error types (`hyper_util`'s
`legacy::Error` vs `hyper::Error`), so there are two thin mappers — `map_legacy_err`
(send) and `map_hyper_err` (body) — sharing the classification below. No panics
(CLAUDE.md); no `Timeout` mapping — semantic timeout is the `Timeout` *layer*; a
connect-timeout is a connection *failure*:

| Source condition | `HttpError` |
| --- | --- |
| `legacy::Error::is_connect()` (DNS/TCP/TLS/handshake), incl. connect-timeout expiry | `Connection(e)` — matches the variant's documented "DNS, TCP, TLS, backend transport" |
| any other send error (protocol, canceled) | `Other(e)` — "network error" |
| mid-stream body `hyper::Error` | `Other(e)` |

### `build()` — one-line delegation

```rust
/// Assemble the canonical resilience stack over a fresh pooled hyper leaf.
///
/// `build(cfg, timer, auth, rate_limits, conn) = stack(hyper_leaf(conn), cfg, timer,
/// auth, rate_limits)`. Bounds mirror `stack()` exactly.
///
/// # Errors
/// [`BuildError`] from `stack()` if `rate_limits` is not total over `K::all()`, a
/// policy is out of range, or the concurrency-singleton invariant is breached.
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

`HyperBody: Send` (`Incoming` is `Send`), satisfying `stack()`'s `S::Body: Send`.

## Delivery: two PRs

**PR A — transport (`feat/net-http-hyper`).** New crate + deps; `TokioTimer`;
`HyperBody`/`HyperLeaf`/`ConnConfig`/`hyper_leaf`; `map_hyper_err`; `build()` (leaf
streams only); loopback tests (plain-HTTP + self-signed TLS). CI-green, `build()`
fully functional for streaming responses.

**PR B — buffering (`feat/net-http-hyper-buffer`, off `main` after PR A merges).**
The leaf reads `req.extensions().get::<BufferMode>()` (default `Stream`, ADR-0030 §4);
`Buffer` → `Incoming.collect().await` → `ResponseBody::buffered(bytes)`. Additive only:
no signature/type/layer change. Tests: a buffered request returns a `Buffered` body of
exact length; a streaming request is unchanged.

## Testing

Deterministic, no external network (dev-deps: `rcgen`, `tokio` `rt`+`macros`+`net`,
`tracing-subscriber`):

- **Plain-HTTP loopback** — a `hyper` server on `127.0.0.1:0`: `send` round-trips a
  body; a server that aborts mid-response → `HttpError::Other`; an unroutable/black-hole
  address → `connect_timeout` fires → `HttpError::Connection`.
- **TLS loopback** — `rcgen` self-signed cert; a rustls loopback server + a test client
  whose `HttpsConnector` roots include that cert: exercises the real aws-lc-rs/webpki
  handshake and a successful `https://` round-trip in CI.
- **`TokioTimer`** — `sleep(d)` elapses ≈ `d` (paused tokio time).
- **`build()` smoke** — assembles over a trivial single-variant `RateLimitConfig` and
  round-trips against the loopback; a `RateLimitConfig` missing coverage → `BuildError`
  (proves delegation to `stack()`'s boot check).
- **Optional `#[ignore]` live-https smoke** — one real host, for manual runs only.

## Docs

- **ADR-0030 §7** — light amendment recording the resolved provider (`aws-lc-rs`),
  roots (`webpki-roots`), and the `connect_timeout` knob. No new ADR; all within §7's
  fixed decision.
- **CHANGELOG** `[Unreleased]` — one entry per PR.
- Crate + item rustdoc; `just doc` in per-task verification (net-http rule).

## Risks / open points

- **`cargo-deny` first contact with the hyper/rustls/aws-lc tree.** New advisories or
  license/ban hits surface here first. Mitigation: run `just deny` early in PR A; a
  hit is a manifest/allowlist decision, not a design change.
- **aws-lc-rs build toolchain in CI.** `aws-lc-sys` needs a C toolchain (cmake, maybe
  nasm). The devcontainer/CI image likely already has it; verify in PR A's first CI run.
