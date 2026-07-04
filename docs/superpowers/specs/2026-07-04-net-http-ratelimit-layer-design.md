# net-http `RateLimit` layer — design (Slice 1, PR 1)

## Context

Slice 0 landed the **construction surface** for the net-http stack: the transport
contract (ADR-0030), `AuthSource`/`Auth`/`Guarded` (PR 3), and the boot-time pacing
*config* — `RateKey`, `LimitPolicy`/`LimitDecl`, the total `RateLimitConfig<K>`, and
`validate_coverage` (PR 4, #72). Those are pure data plus one validator; **no layer
runs yet**.

Slice 1 implements the resilience *layers* from
[ADR-0031](../../adr/0031-http-resilience-venue-pacing.md) — `Timeout`, `RateLimit`,
`Retry`, `CircuitBreaker`, `Tracing` — each as a standalone, composable `Service`
generic over [`net-api::Timer`](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md),
tested over `MockClient` + `MockTimer`. Assembly (`stack()`/`build()`) is Slice 2.
Per the one-issue-one-PR grain Slice 0 held to, **each layer is its own PR**; this
spec covers the **first: `RateLimit`** — the most central layer, wired directly to
the config PR 4 just landed and the one that exercises the most Slice-0 seams
(`Guarded`, `MockTimer`, fail-closed).

### Governing ADRs

- **ADR-0031** §3–§4 — the keyed `RateLimit` layer: rate *xor* concurrency as
  policies, frozen `Arc<HashMap>` buckets, per-bucket `Mutex`, lock-released-before-
  `await`, rate-before-concurrency acquire order, `Throttled` on `max_wait`.
- **ADR-0034** §3 / Amendments (2026-07-04) — `Guarded<B>` carries an
  `Option<SemaphoreGuardArc>` released at the earlier of stream-end or drop;
  `RateLimitConfig<K>` is total over `RateKey::all()` at construction.
- **ADR-0029** — `Timer`, compile-time composition, no `dyn`.

## Goal

A `RateLimit<S, K, T>` `Service` (+ its `Layer` factory) that paces each request
against its global and per-endpoint buckets exactly as the IBKR Client Portal table
requires — proactively, so we **never** hit a 429 — releasing rate permits eagerly
and concurrency permits at transfer end, and failing **closed** (`Throttled`, never
sent) on any coverage gap that reaches runtime.

## Scope (in)

- The `RateLimit<S, K, T>` service + `RateLimitLayer<K, T>` factory (impl'ing
  `net-api::Layer`), in `oath-adapter-net-http-api` (runtime-neutral: `Timer`-generic,
  `async-lock`, `futures-util` for the acquire race — **no** `tokio`).
- The frozen `RateState<K>` bucket structure built from a validated config.
- The `RateScope<K>` per-request extension (`{ scope: Scope, key: Option<K> }`) and
  the `Scope` enum, plus absent-extension and coverage-gap fail-closed handling.
- The **token-refill algorithm** and the **acquire ordering** (rate-before-concurrency,
  global-first), with `max_wait` backpressure.
- Permit lifetime routed through the existing `Guarded<B>` (rate = ZST; concurrency =
  moved into the body).
- The **`LimitPolicy` amendment** — `TokenBucket` gains a `per: Duration` so IBKR's
  sub-1/second limits are expressible — plus the matching `validate_coverage` updates
  and the **≤1-concurrency-permit construction check** (see Decision 6).

## Non-goals (deferred — each its own PR/slice)

| Deferred | Why | Where |
| --- | --- | --- |
| `Timeout`, `Retry`, `CircuitBreaker`, `Tracing` layers | Independent `Service`s; each its own PR | Slice 1 PRs 2–5 |
| `stack()`/`build()` assembly, `HttpConfig`, default layer order | Construction/wiring, not layer behaviour | Slice 2 |
| The `CircuitOpen` `HttpError` variant | Introduced with `CircuitBreaker` | Slice 1 PR 4 |
| Tokio `Timer` impl, hyper backend | Runtime-specific | Slice 2 (`net-http-hyper`) |
| `FixedWindow` / other `LimitPolicy` variants | YAGNI — IBKR needs only `TokenBucket` + `Concurrency` (ADR-0031 §4) | when a venue needs it |
| Multiple concurrency permits per request | `Guarded` holds one; not an IBKR shape (Decision 6) | future `Guarded` generalisation |

## Decisions

### 1. Layer shape & construction

```rust
struct RateLimit<S, K, T>   { inner: S, state: Arc<RateState<K>>, timer: T, max_wait: Duration }
struct RateLimitLayer<K, T> { state: Arc<RateState<K>>, timer: T, max_wait: Duration }

struct RateState<K> { global: Bucket, local: HashMap<K, Bucket> }
enum   Bucket       { Rate(Mutex<TokenState>), Concurrency(Arc<Semaphore>) }
struct TokenState   { tokens: f64, last: Instant }
```

`RateLimitLayer::new(cfg: &RateLimitConfig<K>, timer: T, max_wait: Duration) ->
Result<Self, BuildError>` calls `validate_coverage(cfg)` (and the new concurrency
check, Decision 6), then builds `RateState`: `global` → one `Bucket`; each `local`
key classified `LimitDecl::Policy(p)` → its own `Bucket`; `GlobalOnly` keys get **no**
local bucket (they are global-paced by construction). The `local` map is **frozen**
behind `Arc` — the key set never changes after construction, so lookup is lock-free
and each `Bucket` owns its own `Mutex`/`Semaphore`, scoping contention to a single
endpoint (ADR-0031 §3). `max_wait` is a **layer-level** field (one backpressure
ceiling for the layer), not per-bucket.

### 2. The per-request directive — `RateScope<K>`

```rust
struct RateScope<K> { scope: Scope, key: Option<K> }        // http::Request extension; Clone
enum   Scope        { None, Global, Local, Both }           // Copy
```

Renamed from ADR-0031 §3's `struct RateLimit<K>` sketch to **`RateScope<K>`** so it
does not collide with the layer type also named `RateLimit`. The adapter stamps it
when it builds each request (it knows the endpoint), replacing a classifier closure.

- **Absent extension → fail closed** (`Throttled`, never sent). A forgotten stamp
  must not silently fly global-paced-only, skipping the endpoint's own local
  limit (ADR-0034 Amendment #1); "global only" is an explicit `Scope::Global`.
- **`None` → acquire nothing** — the *explicit* opt-out, the only unlimited path.
- **`Global` / `Local` / `Both`** → the obvious bucket sets.

### 3. Fail-closed on any runtime coverage gap

`validate_coverage` (Slice 0) guarantees every `K::all()` variant is *classified*, but
a request can still reference a bucket that does not exist — e.g. a `GlobalOnly` key
stamped `Local`/`Both` (whose local bucket was never built), or a `Local`/`Both`
directive with `key: None`. Every such gap **fails closed**: the request is rejected
as `HttpError::Throttled`, **never sent**. A silent fail-open would bypass pacing
straight into IBKR's 429 penalty box; only the explicit `Scope::None` is unlimited
(ADR-0031 §3).

### 4. Acquire algorithm

Resolve the required buckets from `(scope, key)`, then acquire in a fixed order —
**all rate tokens first (global then local), then concurrency permits (global then
local)** (ADR-0031 §3). This is the **no-starvation guarantee**: a request never holds
a scarce concurrency permit while merely *waiting* on a rate token.

- **Rate bucket:** lock the `Mutex` → refill (Decision 5) → if `tokens >= 1.0` consume
  one, **unlock**, proceed (the rate permit is a ZST — acquire-and-go); else compute
  the wait until one token accrues, **unlock before the `await`**, `timer.sleep(wait)`,
  and retry. So a throttled request never blocks other acquirers of its bucket.
- **Concurrency bucket:** race `Semaphore::acquire_arc()` against
  `timer.sleep(remaining)` via `futures_util::future::select` (runtime-neutral —
  never `tokio::select!`); the semaphore winning yields a held `SemaphoreGuardArc`,
  the timer winning yields `Throttled`.
- **`max_wait` (backpressure, not failure):** a **single deadline** `timer.now() +
  max_wait` is established once at layer entry and bounds the *whole* acquire — every
  rate wait and the concurrency race are clamped to the remaining budget, so total
  wait never exceeds `max_wait` (not per-phase, which could reach 2×). Reaching the
  deadline with a bucket still exhausted returns `HttpError::Throttled`.

### 5. Token-refill math (`rate + period`)

`LimitPolicy::TokenBucket { rate: u32, per: Duration, burst: u32 }` reads "`rate`
tokens per `per` window". The per-second refill rate is `r = rate / per.as_secs_f64()`.
On each acquire attempt, with `now = timer.now()`:

```text
elapsed = now - state.last
state.tokens = min(burst as f64, state.tokens + elapsed.as_secs_f64() * r)
state.last   = now
```

If `tokens >= 1.0`: `tokens -= 1.0`, proceed. Else `wait = (1.0 - tokens) / r`
seconds. Continuous (fractional) accrual, so `MockTimer.advance(per)` yields exactly
`rate` tokens (capped at `burst`). This is why the config carries `per`: IBKR's
`1/5s` (orders), `1/min` (`/sso/validate`), `1/15min` (scanner) are all
`rate: 1` with `per` of 5 s / 60 s / 900 s — inexpressible under the shipped
`rate: u32` tokens/second (which rejected anything `< 1`).

**Amendment scope:** this changes the shipped `LimitPolicy::TokenBucket` (adds `per`),
its doc-comments, and `validate_coverage` (add `per > Duration::ZERO`); `Duration` is
`Copy + Eq + Hash`, so `LimitPolicy`'s existing derives are unaffected. Recorded as an
append-only amendment to ADR-0034; lands in this PR.

### 6. Permit lifetime → `Guarded`, and the ≤1-concurrency-permit invariant

`RateLimit` **always** returns `http::Response<Guarded<B>>` — one static type, no
caller discipline. Rate permits are ZSTs dropped at acquire. A concurrency permit is
moved into `Guarded::new(body, Some(guard))` after the inner `call` returns;
rate-only / `None` requests yield `Guarded::new(body, None)`. `Guarded` (PR 3)
already releases at the earlier of stream-end or drop: a **buffered** `Full` body
ends on first poll (permit frees promptly — the real IBKR `/history` case), a
**streaming** body holds the permit until transfer end. **No change to `Guarded`.**

**Invariant (documented + enforced):** `Guarded` holds **one** `Option<
SemaphoreGuardArc>`, so a request holds **at most one** concurrency permit. In IBKR
reality the global budget is always *rate* (10/s) and only a local endpoint
(`/history`) is *concurrency*, so `Both` = one rate (ZST) + one concurrency — fits.
A config where `global` is `Concurrency` **and** any `local` key is `Concurrency`
could require two held permits under `Scope::Both`; that is rejected at construction
by a new `BuildError::MultipleConcurrency` (a boot failure, not a silent runtime
permit truncation — consistent with `validate_coverage`'s philosophy). Generalising
`Guarded` to hold several permits is a deferred change (Non-goals).

## Testing (MockTimer-driven)

All timing is driven by `MockTimer.advance()` on a non-tokio executor to demonstrate
runtime-neutrality:

- **Refill exactness:** a `TokenBucket { rate, per, burst }` drained to empty accrues
  exactly one token after `advance(per / rate)` and saturates at `burst` after
  `advance(per)`; a request then proceeds.
- **Sub-1/s rates:** `rate: 1, per: 5s` admits one request, then `Throttled` until
  `advance(5s)`.
- **`max_wait`:** an exhausted bucket whose refill exceeds `max_wait` returns
  `Throttled`; within `max_wait` it waits then proceeds.
- **Acquire order:** a request needing a rate token *and* a concurrency permit does
  not hold the concurrency permit while waiting on the token (pins no-starvation).
- **Concurrency lifetime:** a held permit rides a **streaming** body and frees the
  next acquirer only at stream-end/drop; a **buffered** body frees it promptly.
- **Fail-closed:** a `Local`/`Both` directive for a `GlobalOnly` key, and a
  `Local`/`Both` with `key: None`, both return `Throttled`, unsent (recording
  `MockClient` sees nothing).
- **Directive semantics:** `None` acquires nothing; absent extension fails closed;
  `Both` spends both budgets.
- **Config amendment:** `validate_coverage` rejects `per == 0`; the new concurrency
  check rejects global-`Concurrency` + local-`Concurrency`; every IBKR row round-trips
  through the amended `TokenBucket`.
- **`Guarded` return type:** `RateLimit`'s response body is always `Guarded<B>`
  regardless of scope.

## Dependencies

`oath-adapter-net-http-api` adds **`futures-util`** (runtime-neutral future
combinators for the acquire race) — already a `[workspace.dependencies]` entry, so no
new *workspace* dep, only a new crate-level use (lighter for `cargo-deny`/`machete`
than introducing `futures-lite`). `async-lock` and `http`/`bytes`/`http-body` are
already crate deps. Still **no** `tokio`/`hyper`/`reqwest`/`serde`. Dev-tests use the
existing `MockClient` (`net-http-mock`) and `MockTimer` (`net-mock`).

## Definition of done

- `RateLimit<S, K, T>` + `RateLimitLayer<K, T>` implemented as specified, the
  `RateScope<K>`/`Scope` extension defined, the `LimitPolicy` `per` amendment +
  `validate_coverage`/concurrency-check landed, all with the tests above.
- ADR-0034 gains an append-only note for the `per` field, the `RateScope` rename, and
  the `MultipleConcurrency` boot check.
- `just ci` green (fmt, lint = deny, test + doctests, doc, deny, typos, machete); no
  new warnings; no `unsafe`/`unwrap`/`expect`/indexing in non-test code.
- `CHANGELOG.md` `[Unreleased]` updated.
- Delivered as one issue → one branch (worktree) → one PR (`Closes #N`).

## Open questions (for the implementation plan)

1. **Amendment placement** — does the `LimitPolicy` `per` change land as its own first
   commit (config + validator + tests) before the layer, or interleaved? A
   `writing-plans` concern; leaning config-first so the layer is written against the
   final type.
2. **`futures-lite` vs hand-rolled `poll_fn`** — if `just deny`/`machete` dislikes the
   new dep, a small hand-rolled race avoids it. Decide during implementation.
