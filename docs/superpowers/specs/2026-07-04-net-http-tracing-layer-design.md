# net-http `Tracing` layer — design (Slice 1, PR 5)

## Context

Slice 0 landed the net-http **construction surface** (transport contract ADR-0030;
`AuthSource`/`Auth`/`Guarded` in #66; the boot-time pacing config in #72). Slice 1
implements the resilience *layers* of
[ADR-0031](../../adr/0031-http-resilience-venue-pacing.md) — each a standalone,
composable `Service` generic over
[`net-api::Timer`](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md),
tested over inline service doubles + `MockTimer`. Assembly (`stack()`/`build()`) is
Slice 2.

Landed so far: **PR 1 `RateLimit`** (#76), **PR 2 `Timeout`** (#78), **PR 3 `Retry`**
(#82). This spec covers **PR 5: `Tracing`** — the outermost layer of the ADR-0031
stack — built **concurrently with the `CircuitBreaker` PR (PR 4)**. The two are
independent files implementing the same `Layer`/`Service` contract; they meet only at
the lib.rs re-export lines (a trivial rebase) and at a documented composition contract
(§Decision 8), never in code. It reuses every seam the prior layers established: the
`Layer`/`Service` contracts, `net-api::Timer`, and the inline-double + `MockTimer` test
pattern (net-http-api **cannot** dev-depend on `net-http-mock`'s `MockClient` — that
closes a crate cycle and the two builds' `Service` impls do not unify; `rate_limit.rs`/
`retry.rs`/`body.rs` use inline doubles for exactly this reason).

### Governing ADRs

- **ADR-0031 §6** — `TracingLayer` is **outermost**, on the zero-runtime `tracing`
  facade: one span per logical request covering retries and pacing waits, with `Retry`
  emitting per-attempt events within it. Records method / route / status / `ErrorKind` /
  latency / attempt count; **never** logs auth material or bodies. Always-on but
  pay-per-use — an omitted `TracingLayer` is zero code in a hand-rolled stack; only
  `build()`'s default includes it.
- **ADR-0031 §1** — the default stack `Tracing → CircuitBreaker → Retry → RateLimit →
  Timeout → BufferOrStream → Auth → leaf`. `Tracing` is the first (outermost) `.layer()`,
  so its span spans the entire logical request including every retry and pacing wait.
- **ADR-0014** — the net stack runs in the Adapter process, outside Core's deterministic
  fold, so this layer's output is the **Telemetry** plane: wall-clock machinery metrics
  (latency/throughput), never seq-stamped, lossy-tolerant, never canonical state. The
  layer only *instruments*; aggregation is a subscriber's job.
- **ADR-0029 §4** — `Timer` (`now()` + `sleep()`), compile-time composition, no `dyn`.
  `now()` was added for exactly the elapsed-time reads this layer needs.

## Goal

A `Tracing<S, T>` `Service` (+ its `TracingLayer<T>` factory) that opens one
`tracing` span per logical request — recording method, route, status, `ErrorKind`,
latency, and attempt count — routed to the ADR-0014 Telemetry plane, **structurally
incapable of leaking secrets**, body-transparent, runtime-neutral (`Timer`-generic,
zero-runtime `tracing` facade, **no** `tokio`), and driven deterministically by a fake
clock in tests. Plus the per-attempt instrumentation in `Retry` that makes attempt
count observable.

## Scope (in)

- The `Tracing<S, T>` service + `TracingLayer<T>` factory (impl'ing `net-api::Layer`),
  in `oath-adapter-net-http-api`, new file `trace.rs`.
- **One span** (`info_span!("http.request", …)`) per `call`, attached to the inner
  future via `tracing::Instrument` so downstream events nest under it; the deferred
  fields recorded on completion.
- **Latency** via `Timer::now()` deltas (the layer is `Timer`-generic).
- **Route** = `req.uri().path()` with the **query string dropped**; **method** from
  `req.method()`.
- **Secret-safety by construction**: the layer reads only method + path + status +
  `ErrorKind` + the clock — never headers, never the body.
- **Body-transparency**: `Response = http::Response<B>` returned untouched.
- **Retry instrumentation** in `retry.rs`: per-attempt events + the ambient
  attempt-count record (Decision 6).
- `tracing = { workspace = true }` runtime dep on net-http-api; `tracing-subscriber`
  dev-dep for the capturing test subscriber.
- Capturing-subscriber tests (`MockTimer`-driven, inline doubles), including the
  secret-safety assertion.

## Non-goals (deferred — each its own PR/slice)

| Deferred | Why | Where |
| --- | --- | --- |
| A low-cardinality templated `RouteLabel` request extension (e.g. `/iserver/account/{id}/orders`) | YAGNI — query-stripped `path()` already de-leaks and IBKR's routes are largely static; a clean additive follow-up mirroring `RateScope`/`Retryable`/`RequestTimeout` when an id-bearing route's cardinality first bites | future PR |
| Metric aggregation, exporters, span→metric rollup, sampling policy | ADR-0031 §6 + ADR-0014: the layer only instruments; a subscriber aggregates. Choosing/wiring a subscriber is a process-boot concern | Adapter/Supervisor boot |
| `stack()`/`build()` assembly that makes `Tracing` outermost-by-default | Construction/wiring; also the join point with the concurrent CircuitBreaker PR | Slice 2 |
| Tokio `Timer` impl, hyper backend | Runtime-specific | Slice 2 (`net-http-hyper`) |
| `TimeoutBody`-style mid-stream span/event instrumentation | No streaming venue yet — IBKR is all-buffered (parity with the Timeout spec's deferral) | when a streaming venue lands |
| Recording attempt count as a value *threaded back* from `Retry` to the outer span | Unnecessary coupling — the ambient current-span record (Decision 6) populates the field with none | n/a |

## Decisions

### 1. Layer shape & construction

```rust
pub struct TracingLayer<T> { timer: T }
pub struct Tracing<S, T>   { inner: S, timer: T }
```

`TracingLayer::new(timer: T) -> Self` is **infallible** — nothing to validate (contrast
`RateLimitLayer::new`), no `Result`/`BuildError`. `Clone` and `Debug` are
**hand-written** (not derived), as in `Timeout`/`Retry`: `Debug` uses
`finish_non_exhaustive`; `Clone` bounds `T: Clone` (and, for `Tracing`, `S: Clone`) so
the derives don't demand `Clone`/`Debug` on the inner service. `impl<S, T: Clone>
Layer<S> for TracingLayer<T> { type Service = Tracing<S, T>; … }` clones the `timer`
into each produced service.

### 2. The span — name, fields, cardinality

One span per `call`:

```rust
let span = tracing::info_span!(
    "http.request",
    method = %req.method(),          // recorded at creation (known up front)
    route  = %route,                 // path only, query dropped (Decision 4)
    status      = tracing::field::Empty,   // recorded on completion (Ok)
    error_kind  = tracing::field::Empty,   // recorded on completion (Err)
    latency_us  = tracing::field::Empty,   // recorded on completion
    attempts    = tracing::field::Empty,   // recorded by Retry via the current span (Decision 6)
);
```

- **Static span name** `"http.request"` → low cardinality (safe as a metric key). The
  variable data (`method`, `route`) are **fields**, not part of the name.
- **`info` level** — the always-on span (§6 "always-on but pay-per-use"); the facade
  makes it zero-cost when no subscriber is attached. Per-attempt `Retry` events are
  `debug` (Decision 6) so ordinary telemetry stays lean while drill-down is available.
- `tracing::field::Empty` declares a field recordable later; recording it after the
  await writes to the same span (spans are cheap-to-clone `Id` handles).

### 3. Latency via `Timer::now()`

The layer is **`Timer`-generic** (like `RateLimit`/`CircuitBreaker`, per ADR-0031
Consequences). `Timer::now() -> std::time::Instant`, so:

```rust
let start   = self.timer.now();
let out     = /* inner call, see Decision 5 */;
let elapsed = self.timer.now().saturating_duration_since(start);   // monotonic, panic-free
span.record("latency_us", u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
```

`saturating_duration_since` (never panics on a non-monotonic read) and
`try_from(...).unwrap_or(u64::MAX)` (no `unwrap`/`as`-truncation — honours the
workspace lints) keep it total. Micros, not millis: network latencies span sub-ms to
seconds. Because `MockTimer::advance()` drives `now()`, a test that advances the clock
across the inner call asserts an **exact** recorded latency — the whole point of the
`Timer` seam.

### 4. Route & secret-safety — structural, not a scrub

```rust
let route = req.uri().path();   // NEVER req.uri() (carries ?query) — §6 names query tokens as a leak
```

Secret-safety is a property of **what the layer reads**, not a redaction pass over what
it emits:

- It reads **method**, **path** (query dropped), **status**, **`ErrorKind`**, and the
  **clock**. It never reads `headers()` (no `Authorization`/`Cookie`/API-key), never
  reads or polls the **body**, and drops the URI's query.
- It sits **outermost** — above `Auth`, which stamps credentials *innermost* per
  attempt ([auth.rs](../../../crates/adapter/net/http/api/src/auth.rs)) — so at
  `Tracing`'s position the request has not even been signed yet. Defence in depth: even
  if it had, the layer still never touches headers.

This is why §6 calls `Tracing` "the one place certain not to leak": the guarantee is
enforced by the read surface, and a capturing test locks it in (§Testing).

### 5. Data flow — instrument, then record on completion

```rust
impl<S, T, B> Service<http::Request<Bytes>> for Tracing<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
    // NOTE: no `B: Send` bound (contrast Retry). Nothing of type `B` is held across an
    // await — the sole await is the inner call; `record()` is synchronous — so the
    // response never crosses a yield point and cannot taint the future's `Send`-ness.
{
    type Response = http::Response<B>;
    type Error = HttpError;

    #[allow(clippy::manual_async_fn)]
    fn call(&self, req: http::Request<Bytes>)
        -> impl Future<Output = Result<Self::Response, HttpError>> + Send
    {
        use tracing::Instrument;   // the trait that attaches a span to a future across awaits
        let route = req.uri().path().to_owned();
        let span  = /* Decision 2, using req.method() + route */;
        async move {
            let start = self.timer.now();
            // `.instrument(span.clone())` enters the span on every poll of the inner
            // future — so EVERY event emitted downstream (CircuitBreaker, Retry's
            // per-attempt, …) nests under this one span via context propagation.
            let out = self.inner.call(req).instrument(span.clone()).await;
            let elapsed = self.timer.now().saturating_duration_since(start);
            span.record("latency_us", u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
            match &out {
                Ok(resp) => { span.record("status", resp.status().as_u16()); }
                Err(e)   => { span.record("error_kind", kind_label(e.kind())); }
            }
            out
        }
    }
}
```

- **`Instrumented<F>: Send` iff `F: Send`**, and the `Service` contract already promises
  `S::Future: Send`, so the `+ Send` return bound is preserved.
- **`S: Sync`** because the returned `Send` future borrows `&self` (`&S: Send` needs
  `S: Sync`; `T: Sync` holds via `Timer: Sync`). Same bound `Timeout`/`Retry` carry.
- **`select`-style ordering is irrelevant here** — there is no race; the layer wraps and
  observes, it does not preempt.
- Not `async fn`: the trait requires the returned future be `Send` (only the desugared
  `impl Future + Send` form states it), matching every other layer.

`kind_label(ErrorKind) -> &'static str` is a `const fn` with a **`_` wildcard arm** — a
cross-crate `match` on the `#[non_exhaustive]`
[`ErrorKind`](../../../crates/adapter/net/api/src/error_kind.rs) is *forced* to have one,
which is precisely why the concurrent CircuitBreaker PR adding a circuit-open
classification cannot break this layer.

### 6. Retry instrumentation + attempt count

ADR-0031 §6 wants attempt count recorded and `Retry` emitting per-attempt events. Rather
than thread a count back out of `Retry`'s loop (coupling the layers), use `tracing`'s
**ambient current span**. Inside [retry.rs](../../../crates/adapter/net/http/api/src/retry.rs)'s
loop:

```rust
// per attempt (drill-down; debug so it is pay-per-use):
tracing::event!(Level::DEBUG, attempt, outcome = /* status or kind */, "http.attempt");
// on backoff (try_from, not `as` — the workspace lints reject truncating casts):
let backoff_us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
tracing::event!(Level::DEBUG, attempt, backoff_us, "http.retry.backoff");
// after the loop settles on a final outcome (`attempt` is `Retry`'s existing `u32`):
tracing::Span::current().record("attempts", attempt);
```

- Because the whole inner call runs inside `Tracing`'s instrumented future,
  `Span::current()` inside `Retry` **is** the `"http.request"` span (per the composition
  contract, Decision 8), so `record("attempts", n)` populates that span's field — an
  **always-on outer-span field**, with no direct dependency between the files.
- **Graceful no-op** when unused: with no subscriber, no `Tracing` span (a hand-rolled
  stack without the layer), or a span lacking the `attempts` field, `record` on a
  disabled/absent span/field does nothing. The facade guarantees zero cost. So `Retry`
  stays correct standalone; the field simply fills in when composed under `Tracing`.
- Per-attempt events at **`debug`** keep normal `info` telemetry to one span per request
  while preserving full drill-down when a subscriber enables `debug`.

This is the only change to a previously-merged layer, and it is additive
(`use tracing` + three event/record lines); `Retry`'s existing behaviour and tests are
untouched.

### 7. Error handling

- No new `HttpError` variant — `Tracing` **observes** outcomes, it never originates one.
  It records `status` (Ok) xor `error_kind` (Err) and returns the inner `Result`
  verbatim.
- The concurrent CircuitBreaker PR owns any new `HttpError`/`ErrorKind` variant; this
  PR's `kind_label` wildcard already accommodates it (Decision 5).

### 8. Stack interaction & the composition contract (ADR-0031 §1)

`Tracing → CircuitBreaker → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf`.
`Tracing` is outermost, so its span covers every retry and pacing wait; it is
body-transparent, composing with `RateLimit`'s `Guarded<B>` output without disturbing
the permit lifetime.

**Composition contract (new, documented here):** *`Tracing` owns the single per-request
span; every inner resilience layer emits `tracing` **events**, never opens its own
span.* A rejection or an attempt is a point-in-time fact, not a nested unit of work, so
events are the right shape — and this keeps `Span::current()` at any inner depth resolved
to `"http.request"`, which is what makes Decision 6's ambient `attempts` record land.
Concretely this is a one-line note for the **concurrent CircuitBreaker PR**: emit a
`debug` event on open/half-open/reject, do not open a span. If a future layer violates
this, the degradation is graceful (a wrongly-parented event / a no-op record), never a
panic or a leak.

### 9. Dependencies

- **Runtime:** promote `tracing = { workspace = true }` into net-http-api's
  `[dependencies]` — the crate is the **first consumer** of the workspace dep already
  declared at `Cargo.toml` (the `tracing` facade is a zero-runtime dep: no executor, no
  I/O), so the crate's "no `tokio`/`hyper`/`reqwest`/`serde`" purity rule (lib.rs) is
  intact.
- **Dev:** add `tracing-subscriber` to `[workspace.dependencies]` and net-http-api's
  `[dev-dependencies]`. Dev-only — never in the shipped surface, consistent with the
  existing `tokio` + `oath-adapter-net-mock` dev-deps. `machete`/`deny` stay green.

## Testing (capturing subscriber, `MockTimer` clock, inline doubles)

A test `tracing_subscriber::Layer` captures span attributes (`on_new_span`), later
`record()` calls (`on_record`), and events (`on_event`) into an
`Arc<Mutex<Captured>>`; installed per test via `tracing::subscriber::set_default(...)`
(RAII guard). `#[tokio::test]` is **current-thread**, so the thread-local dispatcher the
`Instrument` context relies on is the test's own — the capture is deterministic. The leaf
is an **inline** `Service` double (no `MockClient` — cycle). Cases:

- **Happy path + body transparency:** a leaf returning `200` immediately → span records
  `method`, `route`, `status = 200`, `latency_us`; the response body `collect()`s to the
  expected bytes (proves `Response<B>` untouched).
- **Error path:** a leaf returning `Err(HttpError::connection(...))` → span records
  `error_kind = "connection"`, **no** `status`; the call returns `Connection` verbatim.
- **Latency is real and exact:** a leaf that `await`s `timer.sleep(d)`; spawn,
  `yield_now`, `advance(d)` → recorded `latency_us == d` in micros (the `Timer` seam).
- **Attempt count + nested events:** an *eligible* request through a
  `RetryLayer`-wrapped leaf that fails-then-succeeds, all inside `TracingLayer` → the
  `"http.request"` span records `attempts == 2`, and two `http.attempt` events are
  captured within it.
- **Secret-safety (the load-bearing test):** a request carrying an `Authorization`
  header **and** a `?token=SUPERSECRET` query → assert the captured spans/events contain
  neither `SUPERSECRET` nor the header value **anywhere**, and that `route` has no `?`.
- **Zero-cost / graceful path:** `Retry` used **without** `TracingLayer` and with no
  subscriber → its `Span::current().record("attempts", …)` is a no-op and existing
  `Retry` behaviour/tests are unchanged (guards Decision 6).

## Definition of done

- `Tracing<S, T>` + `TracingLayer<T>` implemented as specified; `retry.rs` gains the
  per-attempt events + ambient `attempts` record; `lib.rs` gains `pub mod trace;` +
  re-exports (`Tracing`, `TracingLayer`) + a module-doc bullet; all with the tests above.
- `Cargo.toml` (workspace) declares `tracing-subscriber`; net-http-api `Cargo.toml` adds
  `tracing` (dep) + `tracing-subscriber` (dev-dep).
- An append-only **ADR-0034 amendment** records the Tracing refinements (Timer-generic
  latency, query-stripped route, the "inner layers emit events, not spans" composition
  contract, ambient attempt-count) — same pattern #76/#78 used. Exact amendment number
  depends on merge order relative to the concurrent CircuitBreaker PR (open question).
- `CHANGELOG.md` `[Unreleased]` updated.
- `just ci` green — fmt, lint (**deny**), test + doctests, **`just doc`** (intra-doc
  links), deny, typos, machete; no new warnings; no `unsafe`/`unwrap`/`expect`/indexing
  in non-test code.
- Delivered as one issue → one branch (worktree `.claude/worktrees/net-http-tracing`) →
  one PR (`Closes #N`).

## Open questions (for the implementation plan)

1. **ADR amendment placement/number** — record the Tracing refinements as an append-only
   ADR-0034 amendment (the living 2026-07-04 list), as the Timeout/RateLimit PRs did.
   Leaning yes for trail parity. The number is contended by the **concurrent
   CircuitBreaker PR**; whichever merges second renumbers its own amendment — a rebase
   note, not a design issue.
2. **`attempts` field type** — record as `u64` (attempt index) vs also emitting a
   terminal `http.retry.exhausted` event. Leaning the plain field + per-attempt `debug`
   events; a terminal event is additive later if a subscriber wants it.
3. **Capturing subscriber location** — a small reusable capture `Layer` in the test
   module of `trace.rs`, or shared with `retry.rs`'s new attempt-count test via a
   `#[cfg(test)]` helper. Leaning per-file inline (parity with the inline-double
   convention) unless duplication bites.
