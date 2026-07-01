# `net-http` construction surface — design

**Status:** Approved design, pre-implementation.
**Date:** 2026-06-30.
**Crates:** `oath-adapter-net-http-api` (`crates/adapter/net/http/api`),
`oath-adapter-net-http-hyper` (`crates/adapter/net/http/hyper`),
`oath-adapter-net-http-mock` (`crates/adapter/net/http/mock`, **new, dev-only**).

## Context

[ADR-0029](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md)–[ADR-0031](../../adr/0031-http-resilience-venue-pacing.md)
specify the network adapter stack: the transport split over a runtime-neutral
kernel (0029), the HTTP transport contract (0030), and the resilience/pacing layer
stack (0031). Those three ADRs answer every *structural* question — the crate cut,
why `Service` is not a kernel primitive, the bytes-in/streaming-bytes-out body
model, the layer order and its invariants.

This spec closes the **three construction-surface seams** those ADRs left
underspecified — each a place where an implementer must make a type or trait
decision before `net-http-api` compiles:

1. **`AuthSource`** — referenced three times in 0030 (`build()` arg, the `Auth`
   layer, the adapter boundary) but never defined. Its trait shape is a
   `net-http-api` contract.
2. **The concurrency permit's lifetime** — 0031 §3 says a concurrency `Permit` is
   "attached to the response body," but 0030 §3's `ResponseBody<B>` has nowhere to
   carry it; and 0031 §3 names `OwnedSemaphorePermit` (a `tokio` type) inside
   `net-http-api`, which 0030 declares `tokio`-free. A mechanism *and* a
   contradiction.
3. **The `build()` generic surface** — 0030 §8 sketches `build<T: Timer>(…)`, but
   `RateLimit` is generic over the `RateKey`, the keyed rate map's type is unnamed,
   and the bounds are unstated.

### Governing ADRs

- **ADR-0029** — transport split, runtime-neutral kernel, `Timer` contract,
  compile-time `impl`/RPITIT binding (no `dyn`). The `Send`-bounded `impl Future`
  return style and the "abstract only what needs mocking" principle both come from
  here.
- **ADR-0030** — HTTP contract: `Service<http::Request<Bytes>>` → bytes/streaming,
  `ResponseBody<B>`, one concrete `HttpError`, `HttpClient`, the hyper backend, the
  three-tier `build()`. *Amended by this spec* (charter wording, `Auth` variant).
- **ADR-0031** — resilience/pacing: the layer stack and order, retryability-aware
  `Retry`, keyed `RateLimit` (rate *xor* concurrency), `CircuitBreaker`, `Tracing`,
  all generic over `Timer`. *Amended by this spec* (permit type, boot-time coverage).
- **ADR-0003** — adapter anti-corruption: serialisation/typing stays in the
  concrete adapter; the net layer is outward plumbing.
- **ADR-0007** — in-process ⇒ compile-time binding; no runtime `dyn`.
- **CLAUDE.md** — no `unwrap`/`expect`/panic in non-test code; constructors return
  `Result`. Auditable dependency tree under `cargo-deny`.

## Goal

Pin the three contracts so `net-http-api` is fully specified and the whole stack is
deterministically testable off a mock leaf and a mock clock: define `AuthSource`,
the `Guarded<B>` permit-carrying body + the runtime-neutral semaphore choice, and
the `stack()`/`build()` split with boot-time pacing-coverage validation.

## Scope (in)

- **`AuthSource`** trait in `net-http-api`; the `Auth` layer; a `NoAuth` impl; the
  `HttpError::Auth` variant.
- **`Guarded<B>`** response-body newtype carrying `Option<Permit>`; the
  `Permit` enum over a runtime-neutral semaphore (`async-lock`); the
  `RateLimit`-always-returns-`Guarded<B>` discipline and the two release timings.
- **`stack()`** (assembly over an arbitrary leaf, in `net-http-api`) +
  **`build()`** (hyper leaf, in `net-http-hyper`); `HttpConfig` (non-generic) and
  `RateLimitConfig<K>` (the `K`-generic arg); the `RateKey` trait; `BuildError`;
  boot-time total pacing coverage.
- **`oath-adapter-net-http-mock`** — a dev-only crate providing the `MockClient`
  leaf and `MockTimer`, used to regression-test the stack's ordering invariants.
- **ADR reconciliation** — the two amendments to 0030/0031 (below), recorded so the
  landed ADRs are not silently edited.

## Non-goals (deferred — each its own issue/PR)

| Deferred item | Why deferred | Lands with |
| --- | --- | --- |
| Per-layer *algorithms* (token-bucket maths, retry backoff, CB state machine, tracing fields) | Specified by ADR-0031; this spec fixes only the construction-surface contracts they plug into | The per-layer implementation slices |
| The hyper leaf internals (pooled HTTPS connector, rustls wiring, `hyper::Error → HttpError`) | ADR-0030 §7; mechanical given the `HttpClient` seam | The hyper-backend slice |
| `oath-adapter-ibkr` (model ↔ JSON ↔ Bytes, the IBKR `RateKey` enum + pacing table, `tickle` keepalive) | ADR-0003 boundary; the adapter is a separate role crate | The IBKR adapter slices |
| WebSocket transport (`net-ws-api` + backend) | ADR-0029 §"WS is a deliberate later session" | The WS slice |
| `tickle` keepalive mechanism (background task vs layer) | An adapter concern, explicitly "not a net-http layer" (0030 §8); the 0030/0031 contradiction is noted but resolved in the adapter slice | The IBKR adapter slice |

## Decisions

### Seam #1 — `AuthSource`

The named dependency-inversion seam the adapter implements; the `Auth` layer (in
`net-http-api`) calls it innermost, *inside* `Retry`, so it runs once per attempt
against the final, buffered request — which is what makes per-attempt re-signing
(fresh HMAC timestamp/nonce) and current-token stamping correct.

```rust
pub trait AuthSource: Clone + Send + Sync {
    /// Stamp current credentials onto an outgoing request, immediately before send.
    /// Mutates in place (no clone — `Retry` already owns a per-attempt request).
    /// A failure (e.g. token refresh failed) is an `ErrorKind::Auth` `HttpError`.
    fn authorize(&self, req: &mut http::Request<Bytes>)
        -> impl Future<Output = Result<(), HttpError>> + Send;
}

/// IBKR: the local Client Portal gateway holds the session cookie, so there is
/// nothing to stamp. `authorize` returns a ready `Ok(())`.
#[derive(Clone)]
pub struct NoAuth;
```

The `Auth` layer is then `self.auth.authorize(&mut req).await?; self.inner.call(req).await`
— a straight `?`, no conversion.

**Shape — `fn -> impl Future + Send`, not `async fn` (required).** The composed
stack future must be `Send` (multithreaded tokio). `async fn` in a trait yields
`fn -> impl Future` *without* a `Send` bound and offers no clean way to add one;
the explicit desugared form is the only way to *require* `Send`. This matches
`Timer::sleep` (0029 §4) and `HttpClient::send` (0030 §6).

**Operates on `&mut http::Request<Bytes>` (superset).** Covers all four real
schemes: static bearer/API-key (one header insert), HMAC-signed (needs
method/path/the buffered `Bytes` body), async OAuth refresh (the rare path awaits
inline), and cookie-session/no-op. A "produce `HeaderMap`" alternative was rejected:
it allocates a map per call on the hot path *and* cannot sign over the body.

**Error is `HttpError`, not a new `AuthError`/`BoxError`.** `AuthSource` and
`HttpError` both live in `net-http-api`, and the adapter already speaks `HttpError`
(it receives classified errors back from the stack), so returning it adds no
coupling. `HttpError` needs an `Auth` variant regardless (the layer must classify
auth failures for `Tracing`/Core). Returning it directly *deletes* the `From` impl
and the layer's `map_err`. A boxed `#[source]` is added to the `Auth` variant only
if/when a venue's refresh path needs to preserve an underlying chain — not now
(IBKR's `NoAuth` never errors).

**Performance.** On the hot path the future is immediately-ready for static/HMAC/
cookie schemes — a single-state state machine whose first `poll` returns `Ready`,
no heap allocation (RPITIT, not `async-trait`). The `await` of a ready future
collapses to straight-line code; overhead over a hand-written sync call is in the
noise (nanoseconds), dwarfed by the header insert, the HMAC, or the network.

**Static headers vs `Auth` precedence.** `HttpConfig.headers` (the static default
stamp — a trivial `SetHeaders` folded in near `Auth` per 0031 §1) sits **just
outside** `Auth`, so dynamic credentials win on any key collision (`Auth` is the last
writer before the leaf). Pinned in the assembly order so it isn't accidentally
flipped.

### Seam #2 — concurrency permit on the body, runtime-neutral semaphore

**Mechanism — `Guarded<B>` body newtype (in `net-http-api`, owned by the
`RateLimit` layer).** A streaming response returns at *headers*, so a permit held by
the `call` future would release too early; it must ride with the body. `ResponseBody<B>`
(0030 §3) is pacing-agnostic and stays so — `RateLimit` wraps the body in its **own**
newtype. The permit is a plain `Option<async_lock::SemaphoreGuardArc>` — the rate
path holds nothing (token consumed, not held), so a `Permit::Rate` ZST would be
redundant with `None`; only a **concurrency** permit is ever carried (global is
always a rate bucket, local is rate-xor-concurrency, so at most one guard rides the
body even for `Scope::Both`):

```rust
/// Wraps any response body, carrying an optional concurrency permit released at
/// the earlier of stream-end or drop. `Body` is delegated via `pin-project-lite`
/// (no `unsafe`).
struct Guarded<B> { #[pin] inner: B, permit: Option<async_lock::SemaphoreGuardArc> }

impl<B: http_body::Body<Data = Bytes, Error = HttpError>> http_body::Body for Guarded<B> {
    type Data = Bytes;
    type Error = HttpError;
    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        if frame.is_none() { *this.permit = None; }            // eager release at stream-end
        Poll::Ready(frame)
    }
    fn is_end_stream(&self) -> bool { self.inner.is_end_stream() }        // MUST forward
    fn size_hint(&self) -> http_body::SizeHint { self.inner.size_hint() } // MUST forward
}
```

Two non-obvious correctness points the `/* delegate */` shorthand would hide:

- **Forward `is_end_stream` and `size_hint` explicitly.** In http-body 1.x only
  `poll_frame` is required; the other two are provided methods (defaulting to `false`
  and unbounded), and `pin-project-lite` helps only `poll_frame`. Letting the two
  `&self` methods fall back to defaults is **not** a wire-framing bug *here* —
  `Guarded` is a client-side *received-response* body, and hyper settles
  keep-alive/framing at the `Incoming` layer **below** `Guarded`, never seeing it. It
  is a **consumer-side metadata bug**: `BodyExt::collect()` loses its pre-size hint
  (reallocs), streaming consumers lose `is_end_stream`, and — with teeth — any
  **max-response-size guard reading `size_hint().upper()` sees `None` (unbounded) and
  fails open.** The *same forwarding requirement applies to `ResponseBody<B>`
  (0030 §3)*, which has the identical newtype-over-`Either` shape. A test asserts both
  newtypes report `size_hint`/`is_end_stream` identical to their inner body.
- **Release at the earlier of stream-end or drop.** The struct field alone gives
  *drop-only* release — a body read to completion but still held keeps the permit,
  wasting one of IBKR `/history`'s 5. So `poll_frame` `take()`s the permit on the
  terminal frame (`None`), while the field stays for the early-abort case (a cancelled
  read drops the still-`Some` guard). Dropping the guard is synchronous/runtime-free
  (decrement a counter, wake one `event-listener` waiter — safe in both `Drop` and
  `poll_frame`).

`RateLimit` **always** returns `http::Response<Guarded<B>>` (one static type):

- `permit: None` — rate-scoped, unscoped, **or buffered** responses: the concurrency
  permit (if any) is dropped at `call`-return per 0031 §3 (buffered work is done when
  the fetch returns).
- `permit: Some(_)` — a **streaming** concurrency-scoped response (the real IBKR
  `/history` case): the guard rides the body and releases at the earlier of
  stream-end or drop. No caller discipline, no response-extension hand-off (0031
  rejected that).

Final assembled body type: `Guarded<ResponseBody<HyperBody>>`, opaque to adapters
behind `impl HttpClient`.

**Why body-attach, not the response-future (a recorded design strength).** This is
deliberately stronger than tower's stock `ConcurrencyLimit`, which attaches the
permit to the *response future* — for a streaming response that resolves at headers,
so tower would free the `/history` permit *before* the body downloads and under-count
concurrency through the (large) transfer. Body-attach is instead the **hyper
connection-pool model** (`Pooled<T>` ties the checked-out connection to the response
body, returning it on drop). Recorded so nobody "simplifies" it toward the tower
default.

**Semaphore — `async-lock`, not `tokio::sync` (resolves the 0030/0031
contradiction).** 0031 §3 named `OwnedSemaphorePermit` (`tokio`) inside a crate
0030 declares `tokio`-free. Resolution: `net-http-api` depends on
**`async-lock`** (`Permit::Concurrency(SemaphoreGuardArc)`), keeping the contract
crate's dependency graph free of any async runtime.

Rationale, in the order that decided it:

- **A semaphore needs no mock** (unlike `Timer`, which must fake a 15-minute
  cooldown): acquiring permits is deterministic in real time, so tests use the real
  thing. The `Timer`-style trait abstraction (considered option **Q**) buys nothing
  and would thread a generic through `RateLimit` *and* `Guarded<B>` — rejected as
  over-engineering until a second backend supplies its own semaphore (YAGNI).
- **Neither candidate forces a backend's runtime.** The semaphore lives in the
  `RateLimit` *layer*, never the leaf; and both `tokio::sync::Semaphore` and
  `async_lock::Semaphore` are runtime-agnostic (no reactor). So a future non-tokio
  backend is not precluded either way.
- The deciding axis is therefore **declared vs implicit neutrality.** `async-lock`
  keeps `tokio` out of `net-http-api`'s graph, so the graph *states* runtime-
  neutrality rather than relying on the reader knowing `tokio/sync` is reactor-free.
  **Honest cost accounting:** option **P** (`tokio` with `default-features = false,
  features = ["sync"]`) is *not* heavy — that subset is `mio`-free and runtime-free,
  and since `tokio` is already a workspace dep, P would add **zero** new crates. **R**
  adds **two** (`async-lock`, `event-listener`). So R's real price is two small
  smol-rs crates bought purely for *graph-level* neutrality — the technical delta is
  just "does `net-http-api`'s graph name `tokio`." Chosen deliberately on the
  neutrality principle (ADR-0029's runtime-neutral-contract charter), with the thin
  margin recorded so the trade reads straight.

### Seam #3 — the `build()` / `stack()` construction surface

**Split assembly so the ordering invariants are testable.** Every layer
(`Tracing`…`Auth`) already lives in `net-http-api`; only the leaf and `TokioTimer`
are backend-specific. So the canonical assembly lives **once** in `net-http-api`,
over an arbitrary leaf, and the hyper backend delegates:

```rust
// net-http-api — assembles the canonical stack over ANY leaf
pub fn stack<S, T, A, K>(
    leaf: S, cfg: HttpConfig, timer: T, auth: A, rate_limits: RateLimitConfig<K>,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where S: HttpClient + Clone + Send + Sync + 'static, T: Timer, A: AuthSource, K: RateKey;

// net-http-hyper — constructs the hyper leaf, then delegates to stack()
pub fn build<T, A, K>(
    cfg: HttpConfig, timer: T, auth: A, rate_limits: RateLimitConfig<K>, conn: ConnConfig,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where T: Timer, A: AuthSource, K: RateKey;   // = stack(hyper_leaf(conn), …)
```

**Return bound `impl HttpClient + Clone + Send + Sync + 'static`.** Bounding the
opaque return (not just `impl HttpClient`) turns a `Send`/`Clone`/`'static`
regression in any layer into a compile error *at `stack()`* — rather than a cryptic
failure at the adapter's `tokio::spawn`/clone site — and promises adapters the
share/spawn they need. The whole stack satisfies it: every layer holds `Arc` state +
`Clone` config over a `Clone` hyper leaf.

The decisive reason for the split is **not** DRY: several stack properties are
*ordering* invariants, not per-layer behaviours — `CircuitBreaker` *outside* `Retry`
so it never retries open-circuit rejections (0031 §5); `RateLimit` *inside* `Retry`
so each attempt spends budget (0031 §1). No per-layer isolation test can catch a
builder reorder; only a full-stack test over a deterministic leaf
(`stack(MockClient, …, MockTimer)`) can. The assembly must exist once, where the
layers live, so those invariants are regression-testable.

**Config split — non-generic data + the one generic arg.** `HttpConfig` is
non-generic plain data (timeout, retry, headers, circuit-breaker config —
`serde` stays in the adapter). The single `K`-generic is isolated to a separate
`RateLimitConfig<K>` arg (0030 §8 already passes the rate-limiter separately), so
`HttpConfig` carries no type parameter.

**`RateKey` — a typed enum with a finite universe (the K fork, chosen
deliberately).** The fork is real: `K` could be *erased* to a non-generic
`RateKey(u32)`/`&'static str`, dropping the type parameter everywhere — the runtime
fail-closed (below) is what guarantees safety, not the type. We keep `K` **generic**
because a finite enum is the only thing that enables the **boot-time coverage
check**:

```rust
pub trait RateKey: Hash + Eq + Clone + Send + Sync + 'static {
    fn all() -> &'static [Self] where Self: Sized;   // the finite universe
}
```

`Clone` is doubly-earned: `http::Extensions::insert` demands it, and `Retry` clones
the request per attempt (0031 §2) so the `RateLimit<K>` extension's `K` survives
replay. This is documented in the ADR as a *chosen* fork, so nobody "simplifies" K
away and silently drops coverage to runtime-only.

**`all()` must not drift** — the boot-time coverage guarantee is only as trustworthy
as `all()`'s exhaustiveness, and a hand-written slice silently rots when a variant is
added. Two drift-proof options for the adapter (the *impl* side; `net-http-api`'s
trait stays dependency-free): derive it via **`strum::VariantArray`** (an *adapter*
dep, regenerated automatically), or — dependency-free — back `all()` with a slice and
add an **exhaustive-`match` test** (no wildcard arm) that fails to compile when a
variant is added without being listed.

**Boot-time total pacing coverage (hardening beyond 0031 §3).** 0031 §3 validates
config sanity and fails closed at runtime. For the order path that is a missed
hardening: a missing bucket = an unthrottled request = IBKR's 429 → 15-minute IP
penalty box. So `RateLimitConfig<K>` is a **total** map requiring every endpoint to
be *explicitly classified* (rate, concurrency, **or** explicit global-only — not
"absent"):

```rust
enum LimitDecl { Policy(LimitPolicy), GlobalOnly }
struct RateLimitConfig<K> { global: LimitPolicy, local: HashMap<K, LimitDecl> }
```

`build()`/`stack()` validate `local` is total over `K::all()` and return
`Err(BuildError::UndeclaredKey(k))` at construction otherwise. Adding a `RateKey`
variant and forgetting to pace it becomes a **boot failure**, not a first-live-order
429. The runtime `Throttled` fail-closed (0031 §3) stays as an **unreachable
backstop** (defense in depth).

**Return + errors.** Fully opaque `impl HttpClient` — adapters never name
`Tracing<CircuitBreaker<Retry<RateLimit<…, Guarded<ResponseBody<HyperBody>>>>>>`;
they use `Self::Body` through the `http_body::Body` trait. `BuildError` is a
`thiserror` enum (`UndeclaredKey`, bad policy params — rate ≤ 0, burst < 1,
concurrency max < 1 — missing global). No panic (CLAUDE.md).

**Mock leaf behind a dev-only *crate*, not a feature (unification discipline).** A
`default-off` feature is still unification-reachable: if any crate in the graph flips
`net-http-api/mock`, a canned-response leaf becomes constructible in release. A
separate **`oath-adapter-net-http-mock`** crate, depending on `net-http-api` and
pulled only through `[dev-dependencies]`, has **no production dependency edge** — the
feature graph cannot turn it on. It provides `MockClient` (canned responses,
recorded requests) and `MockTimer` (fake clock for the timing layers). The invariant
"the mock cannot exist in production" is enforced by the dependency graph, not by
convention.

### Recorded tradeoffs and deferred refinements

- **No `poll_ready` backpressure (chosen).** The hand-rolled RPITIT `Service`
  ([service.rs](../../../crates/adapter/net/api/src/service.rs)) handles backpressure
  *inside* `call` (awaiting a permit), deliberately dropping tower's `poll_ready`
  readiness — and with it `LoadShed`, `Balance`, and readiness backpressure. A
  non-loss for a single-endpoint signer/retrier (IBKR); named as a known,
  addressable-later limit for a hypothetical high-throughput venue.
- **401-on-`POST` refresh-retry (deferred to the `Retry` slice).** 0031 §2's blanket
  "never retry `POST`" is over-broad for a **definitive 401**: a 401 is *pre-execution*
  (the venue rejected on auth, the order did not execute), so refresh-credentials-and-
  retry is duplicate-safe — unlike an ambiguous timeout. The per-attempt-`Auth`-inside-
  `Retry` order already composes to do it (a `Retry` on 401 re-invokes `authorize`,
  which refreshes). Implement in the `Retry` slice with guards: **once only**,
  **definitive 401 only** (never a timeout/5xx), **venue opt-in**. Not part of this
  construction surface.
- **High-throughput perf cliffs (deferred, YAGNI).** Per-bucket mutex contention,
  `async-lock`-vs-`tokio` semaphore behaviour under load, and per-request span cost are
  all irrelevant at IBKR's ≤10 req/s (network + deliberate pacing waits dominate by
  orders of magnitude; the stack monomorphises to a flat, box-free state machine).
  Named as known limits, addressable when a high-throughput venue lands.

## ADR reconciliation

Recorded as amendments rather than silent edits to landed ADRs (0029–0031 were
landed append-only):

- **ADR-0030 §3 + Consequences** — reword *"free of `tokio`/`hyper`/`reqwest`/`serde`"*
  to *"free of any async **runtime** — and of `hyper`/`reqwest`/`serde`."* The
  concurrency semaphore is `async-lock` (runtime-agnostic), so `net-http-api`'s graph
  names no runtime. Add the `Auth` variant to `HttpError` and `async-lock` to the dep
  list. Specify that `ResponseBody<B>`'s `Body` impl forwards `is_end_stream` and
  `size_hint` to its inner body (the wrapper-transparency rule `Guarded` also needs),
  not only `poll_frame`. And: `HttpError` carries **transport/middleware failures
  only** — HTTP 4xx/5xx statuses are *not* error-ified; they pass through as
  `Ok(Response)` with body intact for the adapter to classify, while `Retry`/
  `CircuitBreaker` peek `status()` + the 429 `Retry-After` header (Slice-0 plan
  refinement — §5's `HttpError` examples were always middleware failures, never
  statuses).
- **ADR-0031 §3** — replace `enum Permit { Rate, Concurrency(OwnedSemaphorePermit) }`
  with a plain `Option<async_lock::SemaphoreGuardArc>` carried by a new `Guarded<B>`
  body newtype (the `Rate` arm was redundant with `None`); specify that `Guarded`
  forwards `is_end_stream`/`size_hint` and eagerly releases on the terminal frame, and
  that body-attach (vs tower's response-future) is the deliberate, stronger guarantee
  for streaming concurrency; correct the §3 wording "stream-end/drop" → "the earlier of
  stream-end or drop"; add the `LimitDecl` total-coverage requirement and the
  `RateKey::all()` boot-time check, demoting the runtime `Throttled` path to an
  unreachable backstop.

## Testing

The construction surface is verified through the mock crate, deterministically:

- **Ordering invariants (the reason `stack()` exists)** — over `MockClient` +
  `MockTimer`: a `CircuitBreaker`-open state rejects *without* `Retry` re-attempting
  it (CB outside Retry); a rate-limited request spends budget on *each* `Retry`
  attempt (RateLimit inside Retry); `BufferMode` survives the `Retry` request clone.
- **`AuthSource`** — `authorize` runs once per attempt (a recording `MockClient`
  asserts the stamped header/signature is present on every attempt, with a fresh
  value per attempt for a signing mock); `NoAuth` is a ready `Ok(())`; an
  `authorize` error surfaces as `ErrorKind::Auth`.
- **Body transparency** — `Guarded` *and* `ResponseBody` report `size_hint` and
  `is_end_stream` identical to their inner body (the metadata-forwarding fix); a
  `size_hint().upper()` size-guard is not silently widened to unbounded by the wrapper.
- **Permit lifetime** — a concurrency-scoped **buffered** response releases its permit
  at `call`-return (`permit: None`); a **streaming** one releases at the *earlier of
  stream-end or drop*: assert the (N+1)-th concurrent acquire unblocks when the N-th
  body is **read to its terminal frame** (eager `take()`), and in a separate test when
  the N-th body is **dropped early** (mid-read); a rate-scoped response carries
  `permit: None`.
- **Acquire fairness** — an ordering test pins the no-starvation behaviour `RateLimit`
  inherits from `async-lock`/`event-listener` (FIFO-ish), recorded as an *inherited*
  property rather than an API guarantee (not strictly depended on at 5 permits, but
  locked so a dep bump can't regress it unnoticed).
- **Boot-time coverage** — `build()` with a `RateLimitConfig` missing a `K` variant
  returns `Err(BuildError::UndeclaredKey)`; a total config builds; bad policy params
  and a missing global are rejected at construction.
- **Production-reachability guard** — a CI assertion that
  `cargo tree -e no-dev -i oath-adapter-net-http-mock` yields no non-dev dependents
  (the mock cannot reach a release build through the feature/dep graph).

## Dependencies

- `oath-adapter-net-http-api` — adds **`async-lock`** (runtime-agnostic semaphore),
  on top of the 0030 deps (`http`, `http-body`, `http-body-util`, `bytes`,
  `pin-project-lite`, `thiserror`, `tracing`). Still **no** `tokio`/`hyper`/`reqwest`/
  `serde`. (`async-lock` pulls `event-listener`/`event-listener-strategy` — small,
  widely-used, runtime-agnostic.)
- `oath-adapter-net-http-hyper` — owns the only `hyper`/`tokio`/`rustls` deps and
  `TokioTimer`.
- `oath-adapter-net-http-mock` — **new**, dev-only; depends on `net-http-api`; pulled
  by other crates only through `[dev-dependencies]`. Provides `MockClient` + `MockTimer`.

All new workspace deps go through `[workspace.dependencies]` per the repo pattern.

## Definition of done

- The three contracts (`AuthSource`, `Guarded<B>` + `Permit` + the semaphore choice,
  `stack()`/`build()` + `RateKey`/`RateLimitConfig<K>`/`BuildError`) are implemented
  as specified, with the mock crate and the tests above.
- The two ADR amendments are landed (as ADR edits or a short follow-up ADR per the
  repo's append-only norm — decided in the implementation plan).
- `just ci` green (fmt, lint = deny, test + doctests, doc, deny, typos, …); no new
  warnings; no `unsafe`/`unwrap`/`expect`/indexing in non-test code.
- `CHANGELOG.md` `[Unreleased]` updated.
- Delivered as one-issue-one-PR slices (the construction surface may split into more
  than one PR; each `Closes` its issue).

## Open questions (for the implementation plan)

1. **ADR form** — fold the two amendments into 0030/0031 directly, or land a short
   ADR-0032 that records them and the construction-surface decisions? (The repo has
   landed 0029–0031 append-only; leaning ADR-0032.)
2. **Slice boundaries** — does the construction surface land as one PR or several
   (e.g. `AuthSource` + `Auth` layer; `Guarded`/semaphore + `RateLimit`;
   `stack`/`build`/coverage; the mock crate)? A `writing-plans` concern.
3. **`MockTimer` home** — confirmed in `net-http-mock` alongside `MockClient`, or a
   thinner test-util tied to `net-api` where `Timer` is defined?
