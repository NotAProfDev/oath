# net-http `Timeout` layer — design (Slice 1, PR 2)

## Context

Slice 0 landed the net-http **construction surface** (transport contract ADR-0030;
`AuthSource`/`Auth`/`Guarded` in #66; the boot-time pacing config `RateKey`/
`RateLimitConfig`/`validate_coverage` in #72). Slice 1 implements the resilience
*layers* of [ADR-0031](../../adr/0031-http-resilience-venue-pacing.md) — `Timeout`,
`RateLimit`, `Retry`, `CircuitBreaker`, `Tracing` — each a standalone, composable
`Service` generic over [`net-api::Timer`](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md),
tested over inline service doubles + `MockTimer`. Assembly (`stack()`/`build()`) is
Slice 2.

**PR 1 landed the `RateLimit` layer** (#76: `RateLimit<S, K, T>` + `RateLimitLayer`,
the `RateScope`/`Scope` directive, the token-bucket + concurrency acquire, `Guarded`
permit lifetime). This spec covers **PR 2: `Timeout`** — the simplest timing layer,
and a clean template for the remaining ones. It reuses every seam PR 1 established:
the `Layer`/`Service` contracts, `net-api::Timer`, `futures_util::future::select` for
the race, and the inline-double + `MockTimer` test pattern (net-http-api **cannot**
dev-depend on `net-http-mock`'s `MockClient` — that closes a crate cycle and the two
builds' `Service` impls do not unify; `rate_limit.rs`/`body.rs` use inline doubles for
exactly this reason).

### Governing ADRs

- **ADR-0031 §1** — the default stack `Tracing → CircuitBreaker → Retry → RateLimit →
  Timeout → BufferOrStream → Auth → leaf`. `Timeout` sits **inside `Retry`** (each
  attempt re-times cleanly) and **outside `BufferOrStream`/`Auth`/leaf**, and it
  *"bounds the send, not the permit wait"* — which is precisely why `RateLimit` is
  outside it.
- **ADR-0029** — `Timer` (`now()` + `sleep()`), compile-time composition, no `dyn`.
- **ADR-0030 §4 / ADR-0034 §2** — `ResponseBody<B>` is `Buffered { Full<Bytes> }` *xor*
  `Streaming { B }`; the buffered branch is fully in memory before the response future
  resolves. This is the fact that makes a body-level timeout unnecessary for v1 (see
  Decision 4).

## Goal

A `Timeout<S, T>` `Service` (+ its `TimeoutLayer<T>` factory) that bounds how long the
inner stack may take to **produce a response**, returning `HttpError::Timeout` when a
per-layer (or per-request-overridden) deadline elapses first — runtime-neutral
(`Timer`-generic, `futures-util` race, **no** `tokio`), body-transparent, and mockable
with a fake clock.

## Scope (in)

- The `Timeout<S, T>` service + `TimeoutLayer<T>` factory (impl'ing `net-api::Layer`),
  in `oath-adapter-net-http-api`.
- The **response-future race**: `select(inner.call(req), timer.sleep(dur))` — inner
  wins → its `Result` verbatim; deadline wins → `HttpError::Timeout` (inner future
  dropped/cancelled).
- The per-request **`RequestTimeout(Duration)`** `http::Request` extension overriding
  the layer default; **absent → layer default** (not fail-closed — see Decision 2).
- **Body-transparency:** `Response = http::Response<B>` passed through untouched (no
  `Guarded`-style wrapper, no body clone).
- `MockTimer`-driven tests with inline service doubles.

## Non-goals (deferred — each its own PR/slice)

| Deferred | Why | Where |
| --- | --- | --- |
| `TimeoutBody<B, T>` — a deadline-carrying body bounding a **streaming** transfer's mid-stream stall | Inert on IBKR's all-buffered traffic (a `Buffered` body is already in memory when `call` returns, so a per-poll deadline can never trip); unspecified by the ADRs. A clean **additive** follow-up when a streaming venue first lands (Decision 4). | future PR |
| `Retry`, `CircuitBreaker`, `Tracing` layers | Independent `Service`s; each its own PR | Slice 1 PRs 3–5 |
| `stack()`/`build()` assembly, `HttpConfig`, default layer order | Construction/wiring | Slice 2 |
| Tokio `Timer` impl, hyper backend | Runtime-specific | Slice 2 (`net-http-hyper`) |
| A separate idle-timeout vs total-deadline distinction | YAGNI — one deadline models every IBKR endpoint | when a venue needs it |

## Decisions

### 1. Layer shape & construction

```rust
pub struct TimeoutLayer<T> { default: Duration, timer: T }
pub struct Timeout<S, T>   { inner: S, default: Duration, timer: T }
```

`TimeoutLayer::new(default: Duration, timer: T) -> Self` is **infallible** — every
`Duration` is a valid deadline, so there is nothing to validate and no `Result`/
`BuildError` (contrast `RateLimitLayer::new`, which validates a config map). `Clone`
and `Debug` are **hand-written** (not derived): `Debug` uses `finish_non_exhaustive`
showing only `default`; `Clone` bounds `T: Clone` (and, for `Timeout`, `S: Clone`) —
the same reason `RateLimit` hand-rolls them, so the derives don't demand `Debug`/`Clone`
on the inner service. `impl<S, T: Clone> Layer<S> for TimeoutLayer<T> { type Service =
Timeout<S, T>; … }` clones the `timer` and copies `default` into each produced service.

### 2. The per-request directive — `RequestTimeout`

```rust
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout(pub Duration);   // http::Request extension
```

`Copy` so it survives the per-attempt request clone `Retry` performs (matching
`Retryability`). The adapter stamps it when it knows an endpoint warrants a
non-default bound (e.g. a heavier fetch). Resolution in `call`:

```rust
let dur = req.extensions().get::<RequestTimeout>().map_or(self.default, |t| t.0);
```

- **Present → the override.** **Absent → the layer default.**
- **Absent is *not* fail-closed** (unlike `RateScope`, ADR-0034 Amendment #1). A missing
  `RateScope` could silently skip an endpoint's own rate limit — a pacing hole into
  IBKR's 429 box — so it is rejected. A missing `RequestTimeout` has no such hazard: the
  layer default still bounds the request. There is no gate to bypass, so the safe
  default is simply "use the global deadline". This asymmetry is deliberate and
  recorded (see §Amendment).

### 3. Data flow — the race

```rust
impl<S, T, B> Service<http::Request<Bytes>> for Timeout<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
{
    type Response = http::Response<B>;
    type Error = HttpError;

    #[allow(clippy::manual_async_fn)]
    fn call(&self, req: http::Request<Bytes>)
        -> impl Future<Output = Result<Self::Response, HttpError>> + Send
    {
        async move {
            let dur  = req.extensions().get::<RequestTimeout>().map_or(self.default, |t| t.0);
            let call = std::pin::pin!(self.inner.call(req));
            let nap  = std::pin::pin!(self.timer.sleep(dur));
            match futures_util::future::select(call, nap).await {
                Either::Left((res, _))  => res,                     // inner first → verbatim
                Either::Right(((), _))  => Err(HttpError::Timeout),  // deadline → Timeout
            }
        }
    }
}
```

- **`select` polls the inner call first**, so a ready inner is never spuriously
  preempted by a `Duration::ZERO` deadline (the ordering `rate_limit`'s `acquire_conc`
  already relies on).
- **`S: Sync`** because the returned `Send` future borrows `&self` (`&S` is `Send` only
  if `S: Sync`; `T: Sync` holds via `Timer: Sync`). Same bound `RateLimit` carries.
- On timeout the inner future is **dropped** — cancellation is the runtime-neutral way
  to abandon the send; no `tokio::time::timeout`.
- Not `async fn`: the trait requires the returned future be `Send` (only the desugared
  `impl Future + Send` form can state it), matching every other layer.

### 4. Why no `TimeoutBody` in v1

The race ends the instant `inner.call()` yields a `Response`. What that bounds depends
on **where the body-read sits relative to that instant**:

- **Buffered response (every real IBKR endpoint — `/history`, `/snapshot`, …).** The
  (future) `BufferOrStream` layer — inside `Timeout` — reads the wire body into
  `Full<Bytes>` **before** returning the `Response`, so the response future already
  covers the entire fetch. Afterward `ResponseBody::Buffered::poll_frame` is a
  synchronous in-memory replay (`Poll::Ready` at once); it **cannot** stall, so a
  body-level deadline could never fire.
- **Streaming response (no current venue).** `inner.call()` returns at headers; the wire
  body is pulled frame-by-frame by the consumer *after* the `Timeout` future has already
  resolved. A mid-stream stall here is the **only** thing a response-future race misses,
  and bounding it is the **only** thing a `TimeoutBody<B, T>` (a body carrying a `Timer`
  + deadline, checked each `poll_frame`) would buy.

Since IBKR is all-buffered and the ADRs mandate nothing here, `TimeoutBody` is inert
weight on 100% of current traffic → deferred. It is a clean additive follow-up (wrap the
`Streaming` arm, leave `Buffered` untouched) when a streaming venue lands.

### 5. Error handling

- Deadline wins → **`HttpError::Timeout`** (existing variant, `→ ErrorKind::Timeout`) —
  **no new variant**.
- Inner `Err(_)` is propagated **unchanged**: a `Connection`/`Auth`/`Throttled` error
  keeps its identity and is never masked as `Timeout`. (`HttpError` has no `PartialEq`,
  so tests assert with `matches!`.)

### 6. Stack interaction (ADR-0031 §1)

`… → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf`. Inside `Retry` so each
attempt gets a fresh deadline; outside `RateLimit` so the deadline bounds the **send**,
not the pacing-permit wait (a request throttled by `RateLimit` returns `Throttled`
before `Timeout` is even entered). `Timeout` is body-transparent, so it composes with
`RateLimit`'s `Guarded<B>` output without disturbing the permit lifetime.

## Testing (MockTimer-driven, inline doubles)

Time is driven by `MockTimer::advance()`; the leaf is an **inline** `Service` double (no
`MockClient` — cycle). `#[tokio::test]` provides the executor; the timeout-firing tests
spawn the call, `yield_now`, then `advance` so the layer's `sleep` resolves while the
inner future is pending (the shape `rate_limit`'s concurrency tests use).

- **Fast inner passes, body transparent:** a leaf returning immediately → `Ok`; the
  response body `collect()`s to the expected bytes (proves `Response<B>` is untouched).
- **Slow inner times out:** a leaf that `await`s `timer.sleep(long)`; layer default `d`
  → spawn, `yield_now`, `advance(d)` → `Err(HttpError::Timeout)`; the leaf's future is
  dropped (assert it did not "complete").
- **Per-request override:** `RequestTimeout(short)` fires before the (longer) default;
  `RequestTimeout(long)` outlives a `default`-length advance — both vs the same slow
  leaf. Absent extension → the default applies.
- **Inner error passes through:** a leaf returning `Err(HttpError::connection(...))` →
  the call returns `Connection`, **not** `Timeout`.
- **`select` ordering:** an immediately-ready leaf with `default = Duration::ZERO` still
  returns `Ok` (inner polled first), not `Timeout`.

## Dependencies

**No new dependency, no `Cargo.toml` change.** `futures-util` (the `select` race) and
`http`/`bytes`/`http-body` are crate deps; `oath-adapter-net-mock` (`MockTimer`) and
`tokio` are dev-deps — all present since #76. Still **no** `tokio`/`hyper`/`reqwest`/
`serde` in the layer.

## Definition of done

- `Timeout<S, T>` + `TimeoutLayer<T>` implemented as specified; the `RequestTimeout`
  extension defined; `lib.rs` gains `pub mod timeout;` + re-exports + a module-doc
  bullet; all with the tests above.
- ADR-0034 gains an append-only amendment for the `RequestTimeout` per-request override,
  the response-future-only scope, and the deferred `TimeoutBody`.
- `just ci` green (fmt, lint = deny, test + doctests, doc, deny, typos, machete); no new
  warnings; no `unsafe`/`unwrap`/`expect`/indexing in non-test code.
- `CHANGELOG.md` `[Unreleased]` updated.
- Delivered as one issue → one branch (worktree) → one PR (`Closes #N`).

## Open questions (for the implementation plan)

1. **ADR placement** — record the Timeout refinements as an append-only ADR-0034
   amendment (the living 2026-07-04 list, where #76 recorded RateLimit as #5), i.e.
   Amendment #6? Leaning yes, for trail-completeness parity with RateLimit. (Note: the
   primary checkout carries an *unmerged* local ADR-0034 edit numbered #5 for unrelated
   WS-auth work; this branch is off `origin/main` where #5 is the RateLimit note, so the
   Timeout amendment is #6 here — any renumbering of the WS-auth edit is that PR's
   concern.)
2. **Test executor** — `#[tokio::test]` + spawn/`yield_now`/`advance` (as `rate_limit`),
   or a hand-polled `Waker::noop()` executor for a stricter runtime-neutrality
   demonstration? Leaning `#[tokio::test]` for parity with the shipped layer's tests.
