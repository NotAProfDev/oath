# net-http response-body bounds — stall `TimeoutBody` + buffered size cap (2026-07-10)

Design for the next **Tier-2** net-http hardening item (tracking issue #102): bound a
misbehaving venue **response body** on its two independent axes — **time** and **memory** —
so a slow or oversized body cannot wedge a scarce concurrency permit or OOM the process.

- **Status:** design — awaiting review, then implementation plan.
- **Scope:** one issue, one PR, one ADR amendment (**ADR-0034 Amendment #13**).
- **Companion audits:** [Fable defect audit](2026-07-05-net-http-audit-findings.md) (N1);
  [deep review](2026-07-06-net-http-deep-review.md) (§8 "no stall timeout on streaming
  permit / buffered body"). Both were **anticipated by ADR-0034** — see Context.

## 1. Context & motivation

Two confirmed findings, on orthogonal axes, both already foreseen in ADR-0034:

- **Stall (time).** In the default `BufferMode::Stream`, the leaf's `call` returns at
  response **headers**; the body then drains *outside* every resilience layer — only
  `Guarded` rides it ([body.rs:140](../../../crates/adapter/net/http/api/src/body.rs#L140)).
  A body that stalls mid-transfer therefore holds one of the venue's concurrency permits
  (IBKR `/history` has only 5) **indefinitely** — nothing bounds it. ADR-0034 **Amendment
  #6** named the fix — a `TimeoutBody` bounding "a *streaming* transfer's mid-stream stall"
  — and **deferred** it ("lands additively when a streaming venue first needs it"). This
  un-defers it.

- **Memory (N1).** `BufferMode::Buffer` collects the whole body in the leaf
  ([leaf.rs:178-182](../../../crates/adapter/net/http/hyper/src/leaf.rs#L178-L182)) with
  **no size cap** — a misbehaving venue can allocate unbounded memory. ADR-0034 **§2**
  built the `ResponseBody`/`Guarded` `size_hint` **transparency** specifically so "any
  `size_hint().upper()` max-size guard" would not "fail open" — i.e. the plumbing for this
  guard was laid deliberately. This wires it.

The two are genuinely complementary: `Buffer` mode is already **time**-bounded (its
`collect()` runs *inside* `call`, so the existing send `Timeout` covers it) but not
**memory**-bounded; `Stream` mode is **memory**-safe (the caller drains frame-by-frame, so
nothing accrues in our code) but not **time**-bounded once `call` returns at headers. So
the stall guard targets **Stream** mode and the size cap targets **Buffer** mode.

## 2. Goals / non-goals

**Goals**

1. A streaming response body that goes idle for longer than a configured duration fails
   with `HttpError::Timeout` and **releases its concurrency permit**.
2. A buffered response body larger than a configured cap fails with a new, non-retryable
   `HttpError::BodyTooLarge` **before** unbounded allocation.
3. Both remain **runtime-neutral** (driven by the `Timer` seam, deterministically
   `MockTimer`-testable) and keep the "every `Body::Error` is `HttpError`" invariant.

**Non-goals (YAGNI / deferred)**

- **Per-request overrides** (`RequestStallTimeout`, per-request byte cap) — additive later
  if a venue needs per-endpoint tuning; v1 is a single config value each.
- **A total (wall-clock) body deadline** — we implement per-frame *inactivity* (stall)
  semantics only, matching `tower_http::TimeoutBody` (its `DeadlineBody` sibling is the
  total-deadline variant; not needed here).
- **Buffer-mode *time* bounding** — stays with the existing send `Timeout` layer.
- **Tier-3 buffering-as-a-layer re-cut** (deep review §5.4) — untouched; this design is
  forward-compatible with it (the `TimeoutBody` is already a layer; the cap moves with the
  buffering when that lands).

## 3. Design overview

| Guardrail | Axis | Mode it protects | Where | Mechanism | Error |
|---|---|---|---|---|---|
| Stall timeout | time | Stream | `net-http-api` (new layer) | per-frame inactivity deadline via `Timer` | `HttpError::Timeout` |
| Buffered size cap | memory | Buffer | `net-http-api` `LimitedBody` + `net-http-hyper` leaf | `size_hint().upper()` fast-fail + byte-count while collecting (typed `HttpError`) | `HttpError::BodyTooLarge` |

## 4. Component A — `Timer` gains an associated `Sleep` type (`net-api`)

The stall body must **store** its sleep future across many `poll_frame` calls (unlike the
`Timeout` layer, which stack-pins one sleep inside a single `async` block). `Timer::sleep`
currently returns an **unnameable** `impl Future`, which cannot be a struct field without
boxing. Adding an associated type makes the concrete future nameable, so the field is
stored **inline** — zero allocation, no `dyn` (mirrors `tower_http::TimeoutBody`'s inline
`Option<tokio::time::Sleep>`).

```rust
// crates/adapter/net/api/src/timer.rs
pub trait Timer: Clone + Send + Sync {
    type Sleep: Future<Output = ()> + Send;          // NEW
    fn sleep(&self, dur: Duration) -> Self::Sleep;   // was `-> impl Future<Output=()> + Send`
    fn now(&self) -> Instant;
}
```

All **three** existing impls already return named, concrete futures — the change is one
line each, and no caller changes (`self.timer.sleep(dur).await` is identical):

| impl | `type Sleep = …` |
|---|---|
| `TokioTimer` ([hyper/src/timer.rs](../../../crates/adapter/net/http/hyper/src/timer.rs)) | `tokio::time::Sleep` |
| `MockTimer` ([mock/src/timer.rs:70](../../../crates/adapter/net/mock/src/timer.rs#L70)) | its existing `pub struct Sleep` |
| `FixedTimer` (net-api test double) | `std::future::Ready<()>` |

Safe because `Timer` is **only ever a generic bound** — there is no `dyn Timer` anywhere in
the workspace (verified), so object-safety is irrelevant. An impl that genuinely could not
name its future may still fall back to `type Sleep = Pin<Box<dyn Future…>>` locally; the
trait *enables* zero-alloc without forcing it. This also equips the forthcoming WS
heartbeat/stall body (ADR-0033) with the same inline-timer storage.

## 5. Component B — `TimeoutBody<B, T>` + `StallTimeoutLayer<T>` (`net-http-api`)

A new module (e.g. `stall.rs`) mirroring `tower_http::timeout::TimeoutBody`, but generic
over our `Timer` and typed to `HttpError`.

```rust
pin_project_lite::pin_project! {
    pub struct TimeoutBody<B, T: Timer> {
        #[pin] inner: B,
        #[pin] sleep: Option<T::Sleep>,   // inline; None until first armed / after each reset
        timeout: Option<Duration>,        // None => fully inert pass-through
        timer: T,
    }
}
```

`poll_frame` (the load-bearing order — poll the timer **before** the body so its waker
stays registered while the body is `Pending`):

1. If `timeout` is `None` → forward straight to `inner.poll_frame` (fully inert).
2. Arm `sleep` if unset: `sleep.set(Some(self.timer.sleep(dur)))`.
3. Poll the timer; if `Ready(())` → `Poll::Ready(Some(Err(HttpError::Timeout)))`.
4. `let frame = ready!(inner.poll_frame(cx));`
5. On any frame (data / trailer / terminal `None` / error), `sleep.set(None)` — **lazy
   per-frame reset** (inactivity semantics; next poll re-arms). Return the frame.

**Transparency (ADR-0034 §2):** forward `is_end_stream` and `size_hint` to `inner` —
required so `Guarded`'s already-ended check and any collector stay correct.

**`StallTimeoutLayer<T>`** mirrors `TimeoutLayer`: its `Service::call` awaits
`inner.call(req)`, then re-wraps the response body:
`Response::from_parts(parts, TimeoutBody::new(body, self.timeout, self.timer.clone()))`.
Requires `B: Body<Data = Bytes, Error = HttpError>`; `TimeoutBody` is `Send` when `B`/`T`
are (preserving the M5 `Body: Send` return bound).

**Inert on Buffer mode:** a buffered body is one ready frame, so the timer arms and
immediately resets — it never fires (matches Am#6's "inert on buffered responses").

## 6. Component C — buffered size cap via a typed `LimitedBody<B>` (`net-http-api`)

A small **typed** body wrapper beside `ResponseBody`/`Guarded` in `body.rs`, kept in
`HttpError` end-to-end. `http_body_util::Limited` exists, but its
`Error = Box<dyn Error + Send + Sync>` forces a runtime downcast and injects a
`Box<dyn Error>`-typed body into the pipeline — breaking the crate's defining "one
concrete `HttpError` for service *and* body" invariant (error.rs). A ~30-line typed
wrapper avoids that and is independently unit-testable:

```rust
pin_project_lite::pin_project! {
    pub struct LimitedBody<B> {
        #[pin] inner: B,
        remaining: u64,
    }
}
// poll_frame: count DATA-frame bytes; on overflow emit HttpError::BodyTooLarge; pass
// trailers / terminal None / inner Err through. is_end_stream forwarded; size_hint
// clamped to `remaining` (mirrors Limited's lower>=n / upper.min(n) logic, no lower>upper).
```

Leaf `Buffer` path — the collect error is **already** `HttpError`, so there is no shim:

```rust
BufferMode::Buffer => {
    let bytes = match self.max_response_bytes {
        Some(cap) => {
            let cap = cap as u64;
            // Fast-fail on an honest oversized Content-Length, before reading a byte.
            if incoming.size_hint().upper().is_some_and(|u| u > cap) {
                return Err(HttpError::BodyTooLarge);
            }
            LimitedBody::new(incoming.map_err(map_hyper_err), cap)
                .collect().await?.to_bytes()          // BodyTooLarge or a mapped inner error
        }
        None => incoming.collect().await.map_err(map_hyper_err)?.to_bytes(),
    };
    ResponseBody::buffered(bytes)
}
```

Peak memory is bounded to ≈ `cap` (the frame that would exceed it is rejected;
`collect()` does not pre-reserve from `size_hint`, so there is no size-hint allocation DoS
— the hyper `to_bytes` #3111 vector). `Stream` mode is untouched (the caller drains).

**Why the stall body is *not* likewise a wrapper we could have reused:** `http_body_util`
has **no** timeout combinator (a pure body-util crate has no clock), and the only
ready-made one, `tower_http::timeout::TimeoutBody`, is welded to `tokio::time::Sleep`
(defeats the `Timer` seam / smol-async goal / `MockTimer` determinism) and drags in the
`tower`/`tower-http` trees this runtime-neutral crate reimplemented `compose.rs` to avoid
(ADR-0029). So both new body wrappers are hand-rolled and typed — consistent with the
existing `ResponseBody`/`Guarded`.

## 7. Error surface — `BodyTooLarge`

- **`net-api`** ([error_kind.rs:14](../../../crates/adapter/net/api/src/error_kind.rs#L14)):
  add `ErrorKind::BodyTooLarge` (non-exhaustive enum, additive).
  - **Non-retryable for free:** `retry.rs::is_transient` matches only `{Timeout, Connection}`
    ([retry.rs:230-231](../../../crates/adapter/net/http/api/src/retry.rs#L230-L231)), so a new
    kind is never retried — no `retry.rs` change. (Correct: retrying an oversized body just
    re-overflows.)
- **`net-http-api`** ([error.rs:18](../../../crates/adapter/net/http/api/src/error.rs#L18)):
  add unit variant `HttpError::BodyTooLarge` (message: "response body exceeded the
  configured maximum") + its `HasErrorKind` arm → `ErrorKind::BodyTooLarge`.
- **`trace.rs`** ([trace.rs:30-40](../../../crates/adapter/net/http/api/src/trace.rs#L30-L40)):
  add `ErrorKind::BodyTooLarge => "body_too_large"`. **Required** — the `_ => "unknown"`
  fallback would otherwise mislabel it (the exact M2-class bug we're avoiding by choosing a
  dedicated variant).

The stall error reuses `HttpError::Timeout` (a body that didn't complete in time *is* a
timeout); it surfaces to the caller during draining (outside the Retry boundary, so Retry
never sees it) and triggers `Guarded`'s permit release.

## 8. Config surface

Both are **new required fields** (pre-release breaking additions — no `Default` on these
structs; every construction site sets them explicitly).

- **`HttpConfig.body_stall_timeout: Option<Duration>`**
  ([stack.rs:30](../../../crates/adapter/net/http/api/src/stack.rs#L30)) — `None` disables
  the stall guard (for a legitimately long-idle streaming endpoint); recommended value
  `Some(Duration::from_secs(30))`. `validate_config` rejects `Some(Duration::ZERO)`
  (a zero stall = insta-stall), symmetric with the existing `timeout` check; `None` is
  allowed.
- **`ConnConfig.max_response_bytes: Option<usize>`**
  ([leaf.rs:81](../../../crates/adapter/net/http/hyper/src/leaf.rs#L81)) — `None` = unbounded
  (today's behaviour); recommended value `Some(16 * 1024 * 1024)`. Leaf-owned because the
  cap is enforced where buffering happens.

Construction sites to update (mechanical): `HttpConfig` in
[stack.rs](../../../crates/adapter/net/http/api/src/stack.rs) (doctest + `http_cfg` helper),
[build.rs](../../../crates/adapter/net/http/hyper/src/build.rs),
[examples/client_with_directives.rs](../../../crates/adapter/net/http/hyper/examples/client_with_directives.rs);
`ConnConfig` in [build.rs](../../../crates/adapter/net/http/hyper/src/build.rs),
[leaf.rs `test_conn`](../../../crates/adapter/net/http/hyper/src/leaf.rs), the example.

## 9. Stack placement & permit-release correctness

`StallTimeoutLayer` is added as the **innermost** `.layer()` in `stack()`
([stack.rs:174-180](../../../crates/adapter/net/http/api/src/stack.rs#L174-L180)):

```rust
let svc = LayerBuilder::new()
    .layer(TracingLayer::new(timer.clone()))                                  // outermost
    .layer(CircuitBreakerLayer::new(cfg.circuit_breaker, timer.clone()))
    .layer(RetryLayer::new(cfg.retry, timer.clone()))
    .layer(rate)                                                              // RateLimit → Guarded
    .layer(TimeoutLayer::new(cfg.timeout, timer.clone()))
    .layer(StallTimeoutLayer::new(cfg.body_stall_timeout, timer))            // NEW innermost
    .wrap(inner);                                                             // SetHeaders(Auth(leaf))
```

New order (outer→inner):
`Tracing(CB(Retry(RateLimit(Timeout(StallTimeout(SetHeaders(Auth(leaf))))))))`.
On the **response** path (inner→outer, `Auth`/`SetHeaders`/`Timeout` all body-transparent):

```
leaf → … → StallTimeout wraps in TimeoutBody → … → RateLimit wraps in Guarded
final body:  Guarded< TimeoutBody< ResponseBody<HyperBody> > >
```

`Guarded` is **outside** `TimeoutBody`, so a stall yields `Some(Err(Timeout))`, `Guarded`
observes a non-`Ok` frame, and its `poll_frame` releases the permit
([body.rs:189-203](../../../crates/adapter/net/http/api/src/body.rs#L189-L203)) — the wedged
`/history` slot is freed. This ordering is the crux of the design and gets a dedicated
full-stack test.

## 10. ADR-0034 Amendment #13

Append-only amendment (highest current is #12):

> **13. Response-body bounds — un-defers Am#6's `TimeoutBody`; wires §2's size guard.**
> The streaming mid-stream-stall `TimeoutBody` deferred in Amendment #6 lands as a
> `StallTimeoutLayer` (innermost, inside `RateLimit` so `Guarded` wraps it): a per-frame
> **inactivity** timeout via the `Timer` seam, `HttpError::Timeout` on stall, inert on
> buffered bodies. To store the sleep future inline (no `Box`/`dyn`), `Timer` gains
> `type Sleep: Future<Output=()> + Send`. Independently, `BufferMode::Buffer`'s collect is
> **capped** (`ConnConfig::max_response_bytes`) via a typed `LimitedBody` wrapper plus a
> `size_hint().upper()` fast-fail, completing the max-size guard §2's wrapper-transparency
> was built to support (N1); overflow is a new non-retryable `HttpError::BodyTooLarge`
> (`ErrorKind::BodyTooLarge`, `error_kind="body_too_large"`). Both config values are
> `Option` (disable-able); `HttpConfig.body_stall_timeout` is `validate_config`-checked
> non-zero when `Some`. Cross-refs: ADR-0031 §1 (`Timeout`), ADR-0030 §4 (buffering).

## 11. Testing strategy (TDD, `MockTimer`-driven)

Red-green per unit, mirroring the existing `Timeout`/`body.rs` test style (inline body
doubles, no `MockClient` dev-dep cycle).

**`TimeoutBody`** (unit, `net-http-api`):
- Stall fires: a body that never yields a frame → advancing past the timeout →
  `Some(Err(HttpError::Timeout))`.
- Steady body does **not** trip: a frame arrives each interval (reset works) → completes OK.
- `timeout = None` → fully inert (never arms; forwards frames verbatim).
- Buffered/already-ended body → inert (arms then immediately resets; never fires).
- `is_end_stream`/`size_hint` forwarded (parity assertion, as in `body.rs`).

**Permit release** (unit + full-stack): a `Guarded<TimeoutBody<…>>` whose inner stalls →
timer advance → permit released while the body is still held (not drop-driven), proving the
ordering; and a full `stack()` test that a wedged streaming `/history` body frees the
concurrency slot.

**Size cap** (leaf, `net-http-hyper`, real hyper server doubles like the existing
truncating server):
- Honest oversized `Content-Length` → upfront `BodyTooLarge`, no body read.
- Lying/absent length streaming past `cap` → `BodyTooLarge` from `collect`.
- Under-cap body → collects normally.
- `None` cap → unbounded (today's behaviour) still works.

**Classification** (`stack()`): `BodyTooLarge` returned from the leaf in `Buffer` mode is
**not retried** (leaf hit once); `error_kind` label is `"body_too_large"`.

**`Timer::Sleep`**: existing `Timeout`/`Retry`/`RateLimit`/`CircuitBreaker` suites re-pass
unchanged (proves the associated-type refactor is behaviour-preserving).

## 12. Housekeeping / definition of done

- **CHANGELOG.md `[Unreleased]`** — a "Breaking (pre-release) — net response-body bounds"
  entry (new `HttpConfig`/`ConnConfig` fields, new `HttpError`/`ErrorKind` variant, `Timer`
  gains `type Sleep`) plus a feature line.
- **Rustdoc + doctests** on every new public item (`TimeoutBody`, `StallTimeoutLayer`,
  `LimitedBody`, `HttpError::BodyTooLarge`, `ErrorKind::BodyTooLarge`, the two config fields,
  `Timer::Sleep`).
- **`just ci` green**, including **`just doc`** (per prior net-http layer PRs — check/lint/test
  miss broken intra-doc links).
- GitHub: open the Tier-2 issue, branch in a worktree under `.claude/worktrees/<slug>`,
  `Closes #N`, squash-merge; tick the "Streaming stall TimeoutBody" box in #102.

## 13. Task breakdown (for the implementation plan)

1. `Timer::Sleep` associated type + 3 impls (+ re-pass existing timing suites).
2. `ErrorKind::BodyTooLarge` + `HttpError::BodyTooLarge` + `HasErrorKind` + `kind_label`.
3. `TimeoutBody` + `StallTimeoutLayer` module (TDD) — the stall unit tests.
4. Wire `StallTimeoutLayer` into `stack()`; `HttpConfig.body_stall_timeout` +
   `validate_config`; full-stack permit-release test.
5. Typed `LimitedBody<B>` wrapper (TDD, `net-http-api`) — its unit tests.
6. `ConnConfig.max_response_bytes` + leaf `Buffer`-arm cap using `LimitedBody` — the cap
   leaf tests.
7. ADR-0034 Amendment #13; CHANGELOG; update construction sites, doctests, the example.

## 14. Risks / open questions

- **Default values** (`30s` stall, `16 MiB` cap) are proposals — confirm against IBKR's
  actual payload sizes / idle behaviour.
- **`LimitedBody` `size_hint` clamp** must not report `lower > upper` — mirror
  `http_body_util::Limited`'s `lower>=n → set_exact` / else `upper.min(n)` logic; a unit test
  on a body whose `size_hint` lower bound exceeds the cap pins it.
- **Lazy reset skew:** re-arming on the next poll (not at frame arrival) measures the fresh
  deadline from that poll — the same minor skew `tower_http::TimeoutBody` accepts; immaterial
  for a stall (inactivity) bound.
