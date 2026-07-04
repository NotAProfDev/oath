# net-http `Retry` layer — design (Slice 1, PR 3)

## Context

Slice 0 landed the net-http **construction surface** (transport contract ADR-0030;
`AuthSource`/`Auth`/`Guarded` in #66; the boot-time pacing config `RateKey`/
`RateLimitConfig`/`validate_coverage` in #72). Slice 1 implements the resilience
*layers* of [ADR-0031](../../adr/0031-http-resilience-venue-pacing.md) — `Timeout`,
`RateLimit`, `Retry`, `CircuitBreaker`, `Tracing` — each a standalone, composable
`Service` generic over [`net-api::Timer`](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md),
tested over inline service doubles + `MockTimer`. Assembly (`stack()`/`build()`) is
Slice 2.

**PRs 1–2 landed `RateLimit` and `Timeout`** (#76: `RateLimit<S, K, T>` +
`RateLimitLayer`, the `RateScope`/`Scope` directive, token-bucket + concurrency
acquire, `Guarded` permit lifetime; #78: `Timeout<S, T>` + `TimeoutLayer<T>`, the
response-future race, the `RequestTimeout` override). This spec covers **PR 3:
`Retry`** — the *order-safe* retry layer. It reuses every seam PRs 1–2 established: the
`Layer`/`Service` contracts, `net-api::Timer`, the `HttpError`/`ErrorKind`
classification, and the inline-double + `MockTimer` test pattern (net-http-api
**cannot** dev-depend on `net-http-mock`'s `MockClient` — that closes a crate cycle and
the two builds' `Service` impls do not unify; `rate_limit.rs`/`timeout.rs`/`body.rs`
use inline doubles for exactly this reason).

### Governing ADRs

- **ADR-0031 §1–§2** — the default stack `Tracing → CircuitBreaker → Retry → RateLimit →
  Timeout → BufferOrStream → Auth → leaf`. `Retry` sits **outside `RateLimit`** (each
  attempt spends fresh pacing budget), **outside `Timeout`** (each attempt is
  individually deadline-bounded), and **inside `CircuitBreaker`** (the breaker counts
  *logical*, post-retry outcomes — a later PR). §2 mandates **order-safe** retry: a
  blind wire retransmit of `POST /order` is a funded incident, so `Retry` is
  retryability-aware and **never retries a 429**.
- **ADR-0029** — `Timer` (`now()` + `sleep()`), compile-time composition, no `dyn`.
- **ADR-0030 §4 / ADR-0034 §2** — `ResponseBody<B>` is `Buffered { Full<Bytes> }` *xor*
  `Streaming { B }`, `BufferMode` decides which **inside** the retry boundary — so a
  buffered outcome is fully materialised before `Retry` sees it and a dropped-then-
  retried response releases cleanly.

## Goal

A `Retry<S, T>` `Service` (+ its `RetryLayer<T>` factory) that re-issues an
**explicitly-eligible** request on a **transient** failure — `HttpError::{Timeout,
Connection}` or a `5xx` response status — with **capped-exponential, full-jitter**
backoff between attempts, up to a configured attempt count; runtime-neutral
(`Timer`-generic, an internal seeded PRNG, **no** `tokio`, **no new dependency**),
body-transparent, and mockable with a fake clock. Everything else — a `POST` with no
opt-in, a 429, a 4xx, an `Auth` error — passes through **unretried**.

## Scope (in)

- The `Retry<S, T>` service + `RetryLayer<T>` factory (impl'ing `net-api::Layer`), in
  `oath-adapter-net-http-api`.
- `RetryConfig` plain-data config (`max_attempts`, `base`, `cap`, `seed`) and an
  **infallible** `RetryLayer::new(cfg, timer)`.
- The **`Retryable`** marker `http::Request` extension — **explicit-only** eligibility:
  absent → the request is **never retried** (fail-safe; tightens ADR-0031 §2, recorded
  as an ADR-0034 amendment).
- The **retry decision**: retry iff eligible **and** attempts remain **and** the outcome
  is a transient error (`ErrorKind::{Timeout, Connection}`) **or** a `5xx` response.
  Never a 429/`Throttled`, other 4xx, `Auth`, or `Other`/`Unknown`.
- **Capped-exponential full-jitter backoff**: `delay = rand[0, min(cap, base·2ⁿ)]`, via
  an internal `SplitMix64` seeded from `cfg.seed`.
- **Body-transparency:** `Response = http::Response<B>` passed through untouched; the
  prior response is **dropped** before a backoff (releasing any `Guarded` permit).
- `MockTimer`-driven tests with inline service doubles and a fixed seed.

## Non-goals (deferred — each its own PR/slice)

| Deferred | Why | Where |
| --- | --- | --- |
| **Total-elapsed retry budget** (a wall-clock cap across all attempts) | Each attempt's *send* is already bounded by the inner `Timeout` and `RateLimit` `max_wait`; a cumulative-latency cap is a clean **additive** follow-up (add a `budget: Duration`, skip a backoff/attempt that would exceed it) when a latency need appears. | future PR |
| **`Retry-After` header parsing** (timing backoff from a 503/429 hint) | 429 is never retried here (§ADR-0031 §5); a 503 `Retry-After` is an additive refinement over the jitter schedule. | future PR |
| **`CircuitBreaker`, `Tracing` layers** | Independent `Service`s; the breaker wraps `Retry`, `Tracing` emits per-attempt events within its span | Slice 1 PRs 4–5 |
| **Streaming mid-stream recovery** | `BufferMode::Stream` hands mid-stream recovery to the adapter (ADR-0031 §1); `Retry` only re-issues on the *response* outcome (status/error), and drops a partial stream on a 5xx retry | adapter |
| **Per-request backoff / attempt overrides** | YAGNI — one layer schedule models every IBKR endpoint; eligibility is the only per-request knob | when a venue needs it |
| **`stack()`/`build()` assembly, `HttpConfig`, Tokio `Timer`** | Construction/wiring / runtime-specific | Slice 2 |

## Decisions

### 1. Layer shape & construction

```rust
pub struct RetryConfig {
    pub max_attempts: NonZeroU32,  // total sends; retries = max_attempts − 1
    pub base: Duration,            // first backoff ceiling (delay drawn from [0, base])
    pub cap: Duration,             // exponential-ceiling clamp
    pub seed: u64,                 // jitter PRNG seed (varied in prod, fixed in tests)
}

pub struct RetryLayer<T> { cfg: RetryConfig, timer: T }
pub struct Retry<S, T>   { inner: S, cfg: RetryConfig, timer: T, rng: SplitMix64 }
```

`RetryLayer::new(cfg: RetryConfig, timer: T) -> Self` is **infallible** — `NonZeroU32`
makes "≥ 1 send" a *type* invariant (no `Result`/`BuildError`, unlike
`RateLimitLayer::new` which validates a config map), and `cap < base` is harmless (the
ceiling simply never grows past `cap`; `min` handles it). `RetryConfig` is `Copy` plain
data. `Clone`/`Debug` on `RetryLayer`/`Retry` are **hand-written** (as
`RateLimit`/`Timeout` do): `Debug` uses `finish_non_exhaustive` showing `cfg`; `Clone`
bounds `T: Clone` (and, for `Retry`, `S: Clone`) so the derives don't demand
`Debug`/`Clone` on the inner service. `impl<S, T: Clone> Layer<S> for RetryLayer<T> {
type Service = Retry<S, T>; … }` seeds a fresh `SplitMix64` from `cfg.seed` into each
produced service.

### 2. Eligibility — explicit-only, fail-safe (`Retryable`)

```rust
#[derive(Debug, Clone, Copy)]
pub struct Retryable;   // presence = the adapter opted this endpoint in
```

A **ZST marker** extension; `req.extensions().get::<Retryable>().is_some()` gates all
retrying. `Copy` so it survives the per-attempt request clone (matching `RateScope`/
`RequestTimeout`/`BufferMode`). **Absent → the request is sent exactly once and its
outcome returned verbatim** — a forgotten stamp disables retry, it never duplicates a
`POST`.

This **tightens ADR-0031 §2**, which defaulted to "retry idempotent *methods*
(`GET`/`HEAD`/`PUT`/`DELETE`), never `POST`". Explicit opt-in is the same fail-closed
move ADR-0034 Amendment #1 made for `RateScope`: safety is *structural* (the adapter,
which knows the endpoint and its idempotency, stamps intent) rather than inferred from
the method — an adapter that adds a non-idempotent `GET`-shaped call, or wants a
specific `POST` retried under a dedup key, states it explicitly. Recorded as an
ADR-0034 amendment (see §Amendment). *(Naming: ADR-0031 sketches a `Retryability`
extension; the marker `Retryable` is used because fail-safe eligibility needs no
"non-retryable" variant — absence already means that.)*

### 3. The retry decision & data flow

```rust
impl<S, T, B> Service<http::Request<Bytes>> for Retry<S, T>
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
            let eligible = req.extensions().get::<Retryable>().is_some();
            let max = self.cfg.max_attempts.get();
            let mut attempt = 1u32;
            loop {
                let outcome = self.inner.call(req.clone()).await;
                let more = eligible && attempt < max;
                match &outcome {
                    Err(e) if more && is_transient(e.kind()) => {}          // fall through → backoff
                    Ok(r)  if more && r.status().is_server_error() => {}    // 5xx → backoff
                    _ => return outcome,                                    // terminal outcome
                }
                drop(outcome);                     // release the prior response's Guarded permit
                self.backoff(attempt).await;       // capped-exponential full jitter (§4)
                attempt += 1;
            }
        }
    }
}

fn is_transient(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Timeout | ErrorKind::Connection)
}
```

- **`req.clone()` per attempt** is a *whole-request* clone. `http::Extensions` requires
  `Clone` on `insert`, so `Request<Bytes>: Clone` (`Bytes` is a cheap refcount bump; the
  `Retryable`/`RateScope`/`RequestTimeout`/`BufferMode` extensions ride along). This is
  the mechanism ADR-0031 §2's "`Copy`, survives replay" and every layer's "survives the
  per-attempt request clone" doc refer to.
- **`Auth` re-stamps for free.** `Auth` is *inside* `Retry`, so every `inner.call`
  re-runs the whole inner stack — `RateLimit` acquires fresh budget and `Auth` stamps
  current credentials on each attempt. `Retry` does nothing special for either; the
  request it clones is the pre-`Auth` original (no stale `Authorization` header to
  strip).
- **`429`/`Throttled` is never retried** — `Throttled` is not in `is_transient`, and a
  429 arriving as an `HttpError::Throttled` (from `RateLimit`'s own `max_wait`) or as a
  status is excluded on both the error and the `5xx`-only status paths (429 is 4xx).
  4xx (`Client`), `Auth`, and `Other`/`Unknown` errors are likewise terminal.
- **`drop(outcome)` before backoff** releases the prior response's `Guarded` permit
  (already released at call-return for a `Buffered` body; cancels a partial `Streaming`
  body on the rare 5xx-streaming case — acceptable, streaming recovery is the adapter's
  job).
- **On exhaustion the *last* outcome is returned verbatim** (the final transient error
  or 5xx response) — no synthesised "retries exhausted" error; the caller sees the real
  failure and its `ErrorKind`.
- **`S: Sync`** because the returned `Send` future borrows `&self` (same bound
  `RateLimit`/`Timeout` carry). Not `async fn`: the trait requires the future be `Send`.

### 4. Backoff — capped-exponential full jitter

```rust
// attempt is 1-based; the n-th backoff (before attempt n+1) uses shift n−1.
let ceil  = self.cfg.base
    .checked_mul(1u32 << (attempt - 1).min(31))   // saturating shift, no overflow
    .unwrap_or(self.cfg.cap)
    .min(self.cfg.cap);
let delay = self.rng.duration_in(ceil);            // full jitter: uniform [0, ceil]
self.timer.sleep(delay).await;                     // between attempts — OUTSIDE Timeout
```

- **Full jitter** (`rand[0, ceil]`, AWS-style) spreads re-issues; the ceiling grows
  `base·2ⁿ` capped at `cap`. `checked_mul`/`min(31)`/`unwrap_or(cap)` keep it panic- and
  overflow-free (no `Duration` overflow reaches the multiply).
- **`SplitMix64`**, internal, seeded from `cfg.seed`: state steps by the golden-ratio
  constant `0x9E37_79B9_7F4A_7C15` via `AtomicU64::fetch_add` (lock-free, `Send + Sync`,
  no `Mutex` held across the `await`), then a finalise-mix; `duration_in(ceil)` maps a
  draw into `[0, ceil]`. Deterministic given `seed` + draw order → reproducible tests;
  production passes a per-process-varied seed. **No `rand` dependency**, no injected
  `Jitter` generic — the RNG is a pure computation, so it needs neither runtime-neutral
  injection (as `sleep` does) nor a third type parameter.
- Backoff `sleep` is **not** deadline-bounded — it is the gap *between* sends, outside
  the inner `Timeout` (which bounds each *send*).

### 5. Error handling

- **No new `HttpError` variant.** The retry decision reads existing `ErrorKind`
  (`Timeout`/`Connection` transient; `Throttled`/`Auth`/`Client`/`Server`/`Unknown`
  terminal) and `http::StatusCode::is_server_error()`.
- A propagated error keeps its **identity** — `Retry` returns the inner `Err(_)`
  unchanged (never re-wraps or masks it). (`HttpError` has no `PartialEq`; tests assert
  with `matches!` and count attempts via the inline leaf.)

### 6. Stack interaction (ADR-0031 §1–§2)

`Tracing → CircuitBreaker → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf`.
`Retry` is **outside `RateLimit`** so each attempt spends fresh pacing budget (a
throttled attempt returns `Throttled` — terminal, never retried), **outside `Timeout`**
so each attempt is independently deadline-bounded (a per-attempt `Timeout` surfaces as a
retryable `HttpError::Timeout`), and **inside `CircuitBreaker`** (Slice 1 PR 4), which
will count the *logical* post-retry outcome. `Retry` is body-transparent, composing with
`RateLimit`'s `Guarded<B>` output (its inner `B`) without disturbing the permit lifetime
— it only ever **drops** a superseded response, releasing that permit.

## Testing (MockTimer-driven, inline doubles)

Time is driven by `MockTimer::advance()`; the leaf is an **inline** `Service` double
that counts calls and yields a scripted sequence of outcomes (no `MockClient` — cycle).
`#[tokio::test]` provides the executor; backoff-firing tests spawn the call,
`yield_now`, then `advance` so the layer's `sleep` resolves while the retry loop is
parked. A **fixed `seed`** makes the jitter sequence deterministic.

- **Not eligible → one send.** No `Retryable` extension + a leaf returning a transient
  error → the call returns that error after **exactly one** leaf call (proves the
  fail-safe default; a bare `POST`-shaped request is never re-issued).
- **Eligible transient error → retries then succeeds.** `Retryable` + a leaf scripted
  `Connection` error then `200` → after advancing through the first backoff, `Ok`; leaf
  called twice.
- **`5xx` → retried; success on a later attempt.** Leaf scripted `503` then `200` → the
  5xx is retried, `Ok` returned; the superseded `503` response is dropped.
- **`429`/4xx → never retried.** Leaf returning `429` (and, separately, `400`) → returned
  after one call, no backoff.
- **`Throttled`/`Auth` error → never retried.** Leaf returning `HttpError::Throttled`
  (and `Auth`) → one call, terminal.
- **Attempts exhausted → last outcome verbatim.** `max_attempts = 3` + a leaf always
  returning `Connection` → three calls, then that `Connection` error returned (not a
  synthesised one).
- **Backoff cadence.** With `base`/`cap` set, assert the loop waits the drawn delay: the
  call stays pending until `advance` covers the (seed-determined) delay, then re-issues.
- **Jitter determinism.** A fixed `seed` yields an asserted draw sequence within
  `[0, ceil]`; the ceiling grows `base·2ⁿ` capped at `cap`.
- **Permit released before retry.** A leaf whose response body holds a `Guarded`-style
  permit (inline `Semaphore`) → a retry only proceeds once the prior permit is dropped
  (proves `drop(outcome)` precedes the next `inner.call`).

## Dependencies

**No new dependency, no `Cargo.toml` change.** `http`/`bytes`/`http-body` are crate
deps; `oath-adapter-net-mock` (`MockTimer`) and `tokio` are dev-deps — all present since
#76/#78. `SplitMix64` is a handful of internal lines (no `rand`); the loop uses a plain
`timer.sleep().await` (no `futures-util::select`). Still **no**
`tokio`/`hyper`/`reqwest`/`serde` in the layer.

## Definition of done

- `Retry<S, T>` + `RetryLayer<T>` + `RetryConfig` + the `Retryable` marker + the internal
  `SplitMix64`, implemented as specified; `lib.rs` gains `pub mod retry;` + re-exports +
  a module-doc bullet; all with the tests above.
- ADR-0034 gains an **append-only** amendment for the `Retry` layer: explicit-only
  `Retryable` eligibility (tightening §2), the transient-error + 5xx trigger set with
  no-retry-429, capped-exponential full-jitter backoff via an internal seeded PRNG, and
  the deferred total-budget / `Retry-After`.
- `just ci` green (fmt, lint = deny, test + doctests, doc, deny, typos, machete); no new
  warnings; no `unsafe`/`unwrap`/`expect`/indexing in non-test code (the PRNG recovers
  nothing that can poison; backoff math is `checked_*`).
- `CHANGELOG.md` `[Unreleased]` updated.
- Delivered as one issue → one branch (worktree `.claude/worktrees/net-http-retry`) →
  one PR (`Closes #N`).

## Open questions (for the implementation plan)

1. **ADR amendment number.** On this branch (off `main` @ `de2e5e4`, which now carries
   #80's WS-resilience refinements) ADR-0034's amendment list runs #1–#7 (#7 = the
   `AuthSource` two-traits note, landed via #80). The `Retry` amendment is therefore
   **#8** here. If any concurrent net-http PR lands an ADR-0034 amendment first, renumber
   on rebase (the same trail-completeness convention #78's Timeout spec used).
2. **`Retryable` vs `Retryability`.** Ship the ZST marker `Retryable` (fail-safe needs no
   negative variant) — leaning yes; revisit only if a future endpoint needs to *override*
   an eligible default to non-retryable (it cannot today, since default is already
   "not").
3. **Test executor.** `#[tokio::test]` + spawn/`yield_now`/`advance` (as
   `rate_limit`/`timeout`), for parity with the shipped layers — leaning yes over a
   hand-polled `Waker::noop()` executor.
