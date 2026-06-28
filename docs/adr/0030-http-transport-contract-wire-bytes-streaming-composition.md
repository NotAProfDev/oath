# HTTP transport contract: untyped wire bytes, streaming-by-composition, unified `HttpError`

[ADR-0029](0029-network-adapter-stack-transport-split-compile-time-composition.md)
placed `Service` and the HTTP-specific contracts in `oath-adapter-net-http-api` over
the transport-neutral kernel. This ADR fixes **what that HTTP `Service` carries** —
the request/response types, the body model, the error model, the leaf seam a backend
implements, and the backend itself — driven by the first [Broker](../../CONTEXT.md),
IBKR's Client Portal Web API. The resilience and venue-pacing layers that wrap this
contract are specified in [ADR-0031](0031-http-resilience-venue-pacing.md).

## Decision

### 1. The HTTP stack speaks untyped wire bytes; typing stays in the adapter

```text
Service<http::Request<Bytes>>  →  http::Response<ResponseBody<B>>
```

The whole net-http stack is pure transport: **bytes in, bytes out**. Typed
request-building and JSON (de)serialisation live **above** the net layer, in the
concrete adapter (`oath-adapter-ibkr`), never in `net-http-api`. This is the ADR-0003
anti-corruption boundary made concrete: IBKR's JSON shapes must not leak into a shared
crate, and `serde` must never become a `net-http-api` dependency. The same stack is
therefore reusable for a [Data Provider](../../CONTEXT.md) whose payloads are not JSON
at all.

### 2. Request body buffered, response body streaming — deliberately asymmetric

The **request** body is a buffered `bytes::Bytes`: REST request payloads are tiny, and
`Retry` (ADR-0031) must be able to **replay** a request, which a consumed stream cannot
do. The **response** body is streaming-capable, so the adapter — not the net layer —
decides whether to buffer or stream a given call (SSE, large historical-bar downloads).
"Buffer it" is the caller invoking `.collect()`, not a property baked into the type.

### 3. `ResponseBody<B>` is a newtype over `Either<Full<Bytes>, B>`

The response body is assembled from `http-body-util` standard parts, not a hand-written
`Body` state machine, but wrapped so the vendor types do not leak into the canonical
contract:

```rust
pub struct ResponseBody<B>(Either<MapErr<Full<Bytes>, fn(Infallible) -> HttpError>, B>);
//                         Buffered = Full<Bytes> (one frame)   Stream = B (live leaf body)
impl<B: http_body::Body<Data = Bytes, Error = HttpError>> http_body::Body for ResponseBody<B> { … }
//   delegating impl via pin-project-lite — no `unsafe` (workspace deny)
```

The newtype (over a public `type` alias) earns its keep: `Either` requires both sides to
share one `Body::Error`, so the `Full<Bytes>` side (`Error = Infallible`) must be
`MapErr`'d up to `HttpError` — plumbing that, as an alias, would have leaked
`Either<MapErr<Full<Bytes>, fn(…)>, B>` into every adapter signature.

### 4. Buffering is a per-request directive, not a stack type — so one client serves both

Encoding buffer-vs-stream in the stack type would force an adapter to build *two*
clients (the type differs), duplicating auth, pool, and rate-limit state. Instead the
choice is data on the request — an `http::Request` extension read by a single
`BufferOrStreamLayer`:

```rust
enum BufferMode { Buffer, Stream }   // Copy — survives Retry's request clone
```

- `Buffer` → the layer awaits inner and **collects the body to `Bytes` right there**; a
  mid-read drop becomes an `Err` the surrounding `Retry` can replay. So the normal JSON
  path keeps **full retry coverage including mid-stream failures**, because the body read
  is part of the retried attempt.
- `Stream` → the layer returns the live body at headers; mid-stream recovery is the
  adapter's job.

One configured client, per-call choice. `BufferMode` is `Copy` so it survives the
request clone `Retry` makes; a test asserts replay preserves it.

### 5. One concrete `HttpError` for service *and* body

`HttpError` is the single error type across the stack — both `Service::Error` and the
`Body::Error` of every body in it (`B: Body<Data = Bytes, Error = HttpError>`). It
implements `HasErrorKind` once. Backends map their native error (`hyper::Error`) into
`HttpError` at the leaf — the anti-corruption point we require regardless — with a boxed
`#[source]` variant preserving detail for logs without leaking the type. `net-http-api`
relaxes the kernel's "no `thiserror`" stance to derive `HttpError`, but stays free of
`tokio`/`hyper`/`reqwest`/`serde`. Errors are **not** generic: bodies are generic for
zero-alloc flow-through, but a single concrete error lets any layer *construct* one
(`Timeout`, retry-exhausted, body-read failure) without nested `enum`-wrappers or
`BoxError`.

### 6. `HttpClient` is a blanket-impl'd `Service` sub-trait with `send`

The named dependency-inversion seam the adapter codes against — but it **is** a
`Service`, so the `Layer` machinery composes it:

```rust
pub trait HttpClient:
    Service<http::Request<Bytes>, Response = http::Response<Self::Body>, Error = HttpError>
{
    type Body: http_body::Body<Data = Bytes, Error = HttpError>;
    fn send(&self, req: http::Request<Bytes>) -> impl Future<…> + Send { self.call(req) }
}
impl<S, B> HttpClient for S
where S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>,
      B: http_body::Body<Data = Bytes, Error = HttpError> { type Body = B; }
```

Backends implement `Service` *once* and are `HttpClient` for free; both the raw leaf
(`Body = Incoming`) and the fully-layered stack (`Body = ResponseBody<Incoming>`)
satisfy it. `send` is sugar over `call`. Per ADR-0029 §5 it is a **compile-time
`impl HttpClient` seam**, not `dyn`.

### 7. Backend: `hyper` + `hyper-util` + `rustls`

The first leaf backend (`oath-adapter-net-http-hyper`) is hyper-util's pooled client
over rustls, **not reqwest**:

- It fits the leaf natively — `hyper_util::client::legacy::Client` takes
  `http::Request<B>` and returns `http::Response<Incoming>` where `Incoming` is *already*
  `http_body::Body<Data = Bytes>`; the leaf is nearly an identity wrap plus
  `hyper::Error → HttpError`. reqwest hands back a `Stream` needing re-wrapping.
- We build our own middleware (auth, retry, rate-limit), so reqwest's batteries
  (redirect/cookie/decompression policy) partly **duplicate** the stack and add implicit
  behaviour the anti-corruption ethos wants explicit.
- Smaller, more auditable dependency tree under the workspace `cargo-deny` gate.

The cost is more wiring (we assemble the pooled HTTPS connector); it is contained behind
the `HttpClient` seam, so swapping to a future `net-http-reqwest` is zero churn to the
stack.

### 8. Default assembled stack, data config, three-tier override

`oath-adapter-net-http-hyper` ships the canonical stack behind a data-driven
constructor:

```rust
pub struct HttpConfig {                       // plain data — no serde here
    pub timeout: TimeoutConfig, pub retry: RetryConfig, pub headers: HeaderMap, /* … */
}
pub fn build(cfg: HttpConfig, timer: TokioTimer, auth: impl AuthSource, …) -> impl HttpClient;
```

- **Data vs dependencies:** `HttpConfig` is pure data; the `Timer`, the `AuthSource`,
  and the keyed rate-limiter (ADR-0031) are passed as separate constructor args —
  behaviour and credentials are not config. `serde` stays in the adapter, which maps its
  own deserialised settings into these structs.
- **Three tiers:** *use* the default (`build`); *add* layers by wrapping the returned
  `impl HttpClient` (e.g. IBKR's effectful session-keepalive `tickle`, which is **not** a
  net-http layer); or *replace/reorder* by assembling `ServiceBuilder` from the public
  parts, with the documented order as reference. Batteries included, batteries removable.

## Considered options

- *Typed request/response API in net-http* — rejected: pulls `serde` and venue JSON
  shapes into the shared crate, breaching ADR-0003; serialisation belongs in the adapter.
- *Buffered `Bytes` response only* — rejected: forecloses SSE / large downloads for no
  benefit once buffering is a one-frame `Either` arm.
- *Always-stream, caller `.collect()`s* — rejected: a streaming response returned at
  headers escapes the `Retry` boundary, so the **common** JSON path loses retry coverage
  on a mid-stream drop. The `BufferMode`-inside-retry design keeps it.
- *Public `type ResponseBody = Either<…>` alias* — rejected: leaks `Either`/`Full`/`MapErr`
  into every adapter signature; the newtype hides the assembly behind a stable name.
- *Generic errors bounded by `HasErrorKind`* — rejected: error-producing layers would need
  wrapper-`enum`s or `BoxError`, and the adapter inherits a `Stack`-deep error type. One
  concrete `HttpError` is simpler and we map the backend error regardless.
- *reqwest backend* — rejected for the *first* backend (fit, control, supply chain above);
  remains a viable future `net-http-reqwest` behind the same seam.

## Consequences

- `net-http-api` gains deps `http`, `http-body`, `http-body-util`, `bytes`,
  `pin-project-lite`, `thiserror` (and `tracing`, ADR-0031), but **not**
  `tokio`/`hyper`/`reqwest`/`serde` — it remains a zero-I/O contract crate.
- `net-http-hyper` owns the only `hyper`/`tokio`/`rustls` dependency and the
  `hyper::Error → HttpError` and `Incoming → ResponseBody` mappings — the anti-corruption
  point where the backend is sealed off.
- The adapter (`oath-adapter-ibkr`) owns `model ↔ JSON ↔ Bytes`, request building via
  `http::request::Builder`, the `AuthSource`, and any effectful session management.

## Relationships

Builds on **ADR-0029** (`Service` in `net-http-api`, compile-time seam, `Timer`).
Enforces **ADR-0003** (serialisation/typing in the adapter, backend sealed at the leaf).
Is wrapped by **ADR-0031** (the resilience/pacing layers and `build()`'s default order).
Glossary unchanged — implementation vocabulary only.
