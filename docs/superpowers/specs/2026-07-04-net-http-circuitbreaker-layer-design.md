# net-http `CircuitBreaker` layer — design (Slice 1, PR 4)

## Context

Slice 0 landed the net-http **construction surface** (transport contract ADR-0030;
`AuthSource`/`Auth`/`Guarded` in #66; the boot-time pacing config `RateKey`/
`RateLimitConfig`/`validate_coverage` in #72). Slice 1 implements the resilience
*layers* of [ADR-0031](../../adr/0031-http-resilience-venue-pacing.md) — `Timeout`,
`RateLimit`, `Retry`, `CircuitBreaker`, `Tracing` — each a standalone, composable
`Service` generic over [`net-api::Timer`](../../adr/0029-network-adapter-stack-transport-split-compile-time-composition.md),
tested over inline service doubles + `MockTimer`. Assembly (`stack()`/`build()`) is
Slice 2.

**PRs 1–3 landed `RateLimit`, `Timeout`, and `Retry`** (#76: `RateLimit<S, K, T>` +
the `RateScope`/`Scope` directive, token-bucket + concurrency acquire, `Guarded` permit
lifetime; #78: `Timeout<S, T>` + `TimeoutLayer<T>`, the response-future race; the
`Retry` branch: `Retry<S, T>` + `RetryLayer<T>`, the `Retryable` marker, order-safe
retry with capped-exponential full-jitter backoff). This spec covers **PR 4:
`CircuitBreaker`** — the **reactive** backstop to `RateLimit`'s proactive guard. It
reuses every seam PRs 1–3 established: the `Layer`/`Service` contracts,
`net-api::Timer`, the `HttpError`/`ErrorKind` classification, and the inline-double +
`MockTimer` test pattern (net-http-api **cannot** dev-depend on `net-http-mock`'s
`MockClient` — that closes a crate cycle and the two builds' `Service` impls do not
unify; `rate_limit.rs`/`timeout.rs`/`retry.rs`/`body.rs` use inline doubles for exactly
this reason).

### Governing ADRs

- **ADR-0031 §5** — the CircuitBreaker decision record, the **source of truth** for
  every behavior below: three states (`Closed`/`Open`/`Half-Open`), `Timer`-driven;
  `CircuitBreakerConfig { failure_threshold, cooldown, throttle_cooldown,
  half_open_probes }`; **Closed → Open** on `failure_threshold` consecutive
  `Connection`/`Server`/`Timeout` (consecutive-count for v1; rolling-window later) **or
  immediately on `Throttled`/429** with the long `throttle_cooldown`; **Open** rejects
  fast with a non-retryable `CircuitOpen`; **Half-Open** admits `half_open_probes`
  (success closes, failure re-opens); a **single per-host breaker**, state shared behind
  `Arc`.
- **ADR-0031 §1–§2** — the default stack `Tracing → CircuitBreaker → Retry → RateLimit →
  Timeout → BufferOrStream → Auth → leaf`. `CircuitBreaker` sits **outside `Retry`**, so
  it counts *logical* (post-retry) outcomes and short-circuits **before** `Retry`/
  `RateLimit`/`Timeout` ever run.
- **ADR-0029** — `Timer` (`now()` + `sleep()`), compile-time composition, no `dyn`. The
  breaker uses **`now()` only** — it never sleeps.
- **ADR-0030 §5 / ADR-0034 §2** — HTTP 4xx/5xx *statuses* are **not** `HttpError`s; they
  flow through as `Ok(http::Response)` with the body intact (the adapter classifies).
  The breaker therefore reads its failure signal from **both** the `Err(HttpError)` path
  (`Connection`/`Timeout`) **and** the response `status()` on the `Ok` path (5xx, 429),
  exactly as `Retry` does.
- **ADR-0034** — the construction-surface ADR the layer PRs append their amendments to;
  this layer adds amendment **#9** (see §Definition of done).

## Goal

A `CircuitBreaker<S, T>` `Service` (+ its `CircuitBreakerLayer<T>` factory) that, having
seen `failure_threshold` consecutive transport failures, **trips open** and thereafter
**fast-rejects** every request with a non-retryable `HttpError::CircuitOpen` — never
touching the inner stack — until a `Timer`-measured `cooldown` elapses, at which point a
bounded number of **Half-Open probes** test recovery (a healthy response closes the
circuit; a failure re-opens it). A `Throttled`/429 trips the circuit **immediately** on
the long `throttle_cooldown` (IBKR's ~15-minute penalty box). Runtime-neutral
(`Timer`-generic, `now()`-only — **no sleep**, **no `futures-util`**, **no new
dependency**), body-transparent, single per-host breaker shared behind `Arc`, and fully
mockable with a fake clock. The highest-consequence logic — the state machine — lives in
a **pure, clock-injected `Breaker` unit** that is table-testable with zero async.

## Scope (in)

- The `CircuitBreaker<S, T>` service + `CircuitBreakerLayer<T>` factory (impl'ing
  `net-api::Layer`), in `oath-adapter-net-http-api`, in a new `circuit_breaker.rs`.
- `CircuitBreakerConfig` plain-`Copy` data (`failure_threshold: NonZeroU32`, `cooldown`,
  `throttle_cooldown`, `half_open_probes: NonZeroU32`) and an **infallible**
  `CircuitBreakerLayer::new(cfg, timer)`.
- The **pure `Breaker` core** (`Breaker` + `BreakerState`): clock-injected transitions
  `admit(&mut self, now) -> Admit` and `record(&mut self, class, now)` — no async, `now`
  an input, fully table-tested.
- The **outcome classifier** `classify(&Result<Response<B>, HttpError>) -> Class` — a
  pure, state-independent 4-way partition (Failure / TripNow / Ignored / Success, §3).
- The **new error** `HttpError::CircuitOpen` and its classification `→
  ErrorKind::CircuitOpen` (a **new** variant on the `net-api` `ErrorKind` enum),
  non-retryable.
- The **thin async shell**: `call()` decides admission under a short lock (using
  `timer.now()`), **releases the lock**, runs `inner.call(req).await` on admission (or
  returns `CircuitOpen` on rejection without touching the leaf), then records the
  classified outcome under a second short lock. The lock is **never** held across the
  `await`.
- `MockTimer`-driven tests: pure-core table tests + Service integration over inline
  service doubles.

## Non-goals (deferred — each its own PR/slice or a documented future)

| Deferred | Why | Where |
| --- | --- | --- |
| **`Unknown` → Failure (resilience4j fail-safe default).** v1 treats `Other`/`Unknown` as **Ignored** (conservative — never over-trip the whole gateway on an error we can't explain). | A sustained run of *unclassified* errors arguably signals a host problem; resilience4j's default records all exceptions as failures unless ignored. Deferred until we have evidence of what actually produces `HttpError::Other` from the hyper leaf — reclassifying is a one-line `classify` change plus a table-test row. | future PR |
| **Rolling-window failure counting** (a failure *rate* over a sliding window) | ADR-0031 §5 pins **consecutive-count for v1**; a window is an additive refinement to the `Closed` state's counter and its `record` arm. | future PR |
| **Per-key / per-endpoint breakers** | ADR-0031 §5: IBKR's penalty box is **per-IP, venue-wide**, so **one breaker per host** matches reality (v1). A keyed breaker mirrors `RateLimit`'s `RateKey` map if a venue ever needs per-route isolation. | when a venue needs it |
| **A breaker-state observation surface / watch** (exposing Open/Closed to Telemetry or a trading-halt consumer) | YAGNI for HTTP v1 — the WS stack's inverting breaker relocates the "break" to a risk-layer halt via a lifecycle watch (ADR-0033 §7), but the HTTP breaker (ADR-0031 §5) has no such consumer yet. `Tracing` (PR5) reads per-request outcomes; a watch is additive. | future PR / PR5 |
| **`stack()`/`build()` assembly, `HttpConfig`, Tokio `Timer`** | Construction/wiring / runtime-specific | Slice 2 |
| **`Tracing` layer** | Independent `Service`, outermost; emits per-request spans over the breaker's decision | Slice 1 PR 5 |

## Decisions

### 1. Layer shape & construction

```rust
pub struct CircuitBreakerConfig {
    pub failure_threshold: NonZeroU32,   // consecutive failures in Closed → Open
    pub cooldown: Duration,              // general outage, e.g. 30s
    pub throttle_cooldown: Duration,     // penalty box ≈ 15 min, on Throttled/429
    pub half_open_probes: NonZeroU32,    // probes admitted per Half-Open episode (1)
}

pub struct CircuitBreakerLayer<T> { breaker: Arc<Mutex<Breaker>>, timer: T }
pub struct CircuitBreaker<S, T>   { inner: S, breaker: Arc<Mutex<Breaker>>, timer: T }
```

- **`CircuitBreakerLayer::new(cfg, timer) -> Self` is infallible.** `NonZeroU32` on
  `failure_threshold` and `half_open_probes` makes "≥ 1" a *type* invariant — a `0`
  threshold is nonsense and `0` probes would leave a tripped circuit **stuck Open
  forever** (nothing could ever be admitted to close it). This is the same type-safety
  move `RetryConfig` made with `NonZeroU32` (which needs no `Result`, unlike
  `RateLimitLayer::new`'s config-map validation). **This is a deliberate divergence from
  ADR-0031 §5's sketch**, which types both as `u32`; recorded in the ADR-0034 amendment.
  `CircuitBreakerConfig` is `Copy` plain data.
- **Single per-host breaker, shared.** `new` constructs the `Breaker` **once** into an
  `Arc<Mutex<Breaker>>`; `Layer::layer` **clones the `Arc`** into every produced service,
  and `Clone` for `CircuitBreaker`/`CircuitBreakerLayer` clones the `Arc` (shared state,
  bound `T: Clone`) — so every service the layer yields, and every clone the stack makes,
  observes **one** breaker. This realises ADR-0031 §5's "single per-host breaker, state
  shared behind `Arc`".
- `Debug`/`Clone` on `CircuitBreakerLayer`/`CircuitBreaker` are **hand-written** (as
  `RateLimit`/`Timeout`/`Retry` do): `Debug` uses `finish_non_exhaustive`; `Clone` bounds
  `T: Clone` (and, for `CircuitBreaker`, `S: Clone`) so the derives don't demand
  `Debug`/`Clone` on the inner service.

### 2. The pure core — `Breaker` + `BreakerState` (clock-injected, table-tested)

The state machine is the highest-consequence logic in the layer, so — mirroring how
`Retry` isolated `SplitMix64` and how the WS resilience spec isolates `ReconnectPolicy`
as a pure, table-tested unit — it lives in a **standalone `Breaker`** whose transitions
are **pure functions with `now: Instant` as an input** (the async shell owns the `Timer`
and feeds it). Zero async, zero locks inside — the `Mutex` lives in the shell.

```rust
enum BreakerState {
    Closed   { consecutive_failures: u32 },
    Open     { reopen_at: Instant },                 // cooldown target instant
    HalfOpen { probes_left: u32, successes_needed: u32 },
}
enum Admit { Pass, Reject }

struct Breaker { state: BreakerState, cfg: CircuitBreakerConfig }
```

- **`admit(&mut self, now: Instant) -> Admit`** — the fast-path gate, called before the
  inner send:
  - **`Closed`** → `Pass` (state unchanged).
  - **`Open { reopen_at }`** → if `now >= reopen_at`, transition to `HalfOpen {
    probes_left: half_open_probes − 1, successes_needed: half_open_probes }` and return
    `Pass` (**this** call becomes the first probe); else `Reject`.
  - **`HalfOpen { probes_left, .. }`** → if `probes_left > 0`, decrement and `Pass`; else
    `Reject` (the **concurrency gate** — no more than `half_open_probes` in-flight
    probes; with `half_open_probes = 1`, exactly one at a time).
- **`record(&mut self, class: Class, now: Instant)`** — applied to the *current* state
  after the outcome resolves:

  | State \ Class | `Failure` | `TripNow` | `Ignored` | `Success` |
  | --- | --- | --- | --- | --- |
  | **Closed** | `failures += 1`; at `failure_threshold` → `Open { now + cooldown }` | `Open { now + throttle_cooldown }` | no-op (streak untouched) | `failures = 0` |
  | **Half-Open** | `Open { now + cooldown }` | `Open { now + throttle_cooldown }` | `successes_needed −= 1`; at 0 → `Closed{0}` | `successes_needed −= 1`; at 0 → `Closed{0}` |
  | **Open** | no-op | no-op | no-op | no-op |

  - **Half-Open treats `Ignored` and `Success` identically** — both *resolve the probe*
    and count toward closing. A probe's question is narrowly *"is the host reachable and
    responding coherently?"*; a `4xx`/`Auth` answers **yes** (see §3), so it closes even
    though the same outcome is a no-op in `Closed`. This is what avoids a **stuck
    Half-Open** — every admitted probe reaches a decisive resolution.
  - **`Open` records are no-ops (defensive).** Because the shell releases the lock across
    the `await` (§4), a call admitted while `Closed`/`Half-Open` can have its outcome
    recorded *after* a concurrent call already tripped the circuit; recording into `Open`
    is simply dropped. This loses at most one data point per race and can never
    *un*-trip a freshly-opened circuit — acceptable for a single global v1 breaker.

### 3. Outcome classification — the 4-way partition

```rust
enum Class { Failure, TripNow, Ignored, Success }

fn classify<B>(outcome: &Result<http::Response<B>, HttpError>) -> Class {
    match outcome {
        Err(e) => match e.kind() {
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Server => Class::Failure,
            ErrorKind::Throttled => Class::TripNow,
            // Auth, Client, Unknown, CircuitOpen — and any future kind — are Ignored:
            _ => Class::Ignored,
        },
        Ok(resp) => {
            let s = resp.status();
            if s == http::StatusCode::TOO_MANY_REQUESTS { Class::TripNow } // 429
            else if s.is_server_error() { Class::Failure }                 // 5xx
            else if s.is_client_error() { Class::Ignored }                 // 4xx (non-429)
            else { Class::Success }                                        // 2xx / 3xx
        }
    }
}
```

| Class | Outcomes | Rationale |
| --- | --- | --- |
| **Failure** | `Err(Connection\|Timeout)`, `Ok(5xx)` | Genuine transport / server failure — the signal the breaker exists to count (ADR-0031 §5's "consecutive `Connection`/`Server`/`Timeout`"; `Server` = a 5xx status on the `Ok` path, since `HttpError` never carries `Server`). |
| **TripNow** | `Err(Throttled)`, `Ok(429)` | IBKR's penalty box — trip **immediately** on the long `throttle_cooldown`; retrying/continuing compounds the ban (ADR-0031 §5). |
| **Ignored** | `Ok(4xx non-429)`, `Err(Auth)`, **`Err(Other)`/`Unknown`** | The host answered coherently (4xx) or the fault is caller/credential-side (`Auth`), or unclassified: **do not trip, and do not reset** the failure streak. Not resetting is the key improvement — it stops a `5xx,4xx,5xx,4xx…` interleave from masking a building outage. Ignoring `Auth` is essential: an expired token must **not** trip the whole gateway (refresh is the `Auth` layer's job). `Unknown → Ignored` is the conservative v1 default (see Non-goals for the fail-safe alternative). |
| **Success** | `Ok(2xx\|3xx)` | The host is genuinely healthy → reset the `Closed` streak / resolve a Half-Open probe. |

The `_ => Class::Ignored` catch-all on the `Err` arm is **deliberate and
self-documenting**: anything not explicitly a transport `Failure` or a `TripNow`
defaults to *Ignored* — the conservative "never over-trip on something we didn't
specifically classify" stance, consistent with the `Unknown → Ignored` decision. (Today
`HttpError::kind()` yields only `Timeout`/`Connection`/`Throttled`/`Auth`/`Unknown`; the
`Server`/`Client`/`CircuitOpen` arms are unreachable-via-`HttpError` but keep `classify`
total and correct if the mapping ever widens.)

This partition is grounded in mature circuit-breaker practice — **resilience4j**'s
`recordExceptions`/`ignoreExceptions`, **Polly**'s `HandleTransientHttpError`
(5xx/408/`HttpRequestException` only), and **Envoy**'s consecutive-gateway-errors — all
of which distinguish an *ignored* outcome (neither success nor failure) from a healthy
one, precisely so client-side errors neither trip nor mask.

### 4. The async shell — `call()` data flow

```rust
impl<S, T, B> Service<http::Request<Bytes>> for CircuitBreaker<S, T>
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
            let now = self.timer.now();
            let admit = self.lock().admit(now);        // short lock; released here
            if let Admit::Reject = admit {
                return Err(HttpError::CircuitOpen);    // fast reject — leaf untouched
            }
            let outcome = self.inner.call(req).await;  // NO lock held across the await
            let class = classify(&outcome);
            self.lock().record(class, self.timer.now()); // short lock; released here
            outcome
        }
    }
}
```

- **The lock is a `std::sync::Mutex` held only around the two pure `Breaker` calls,
  never across the `await`** — the same discipline `RateLimit` uses for its bucket
  `Mutex`. A poisoned lock is recovered with
  `.unwrap_or_else(std::sync::PoisonError::into_inner)` (never `.lock().unwrap()`), per
  the workspace no-`unwrap` rule; the `Breaker` holds no invariant a panic could
  corrupt, so recovering is safe.
- **A rejected request never reaches the leaf** — `CircuitOpen` is synthesised locally.
  This is the whole point: an Open circuit stops load cold, protecting a struggling (or
  penalty-boxed) host.
- **Body-transparent** — an admitted request's `http::Response<B>` is returned untouched
  (no `Guarded`-style carrier, no `B` bound beyond the inner `Service`'s); the breaker
  only *reads* `status()` via `classify`.
- **`S: Sync`** because the returned `Send` future borrows `&self` — the same bound
  `RateLimit`/`Timeout`/`Retry` carry.
- **No sleep, no race.** Unlike `Timeout` (races a `Timer::sleep`) and `Retry` (sleeps
  between attempts), the breaker only ever *reads* `timer.now()`; the Open→Half-Open
  transition is a **lazy comparison on the next `admit`**, so there is no background
  timer, no `futures-util::select`, and no new dependency.
- **Cancellation-safe.** An admitted call arms an RAII `ProbeGuard` around
  `inner.call(req).await`; if that future is **dropped** (caller cancellation via
  `select!`/`timeout`) or the inner service **panics** before the real outcome is
  recorded, the guard's `Drop` calls `Breaker::on_abandoned_probe` (Half-Open →
  reopen on a fresh `cooldown`; a no-op in `Closed`/`Open`). The guard is disarmed the
  instant `inner.call` returns normally, so a completed call still records its true
  outcome. This closes the one wedge the "every admitted probe reaches a decisive
  resolution" invariant (§2) didn't cover on its own: a **cancelled** Half-Open probe
  now self-heals after `cooldown` instead of permanently stranding the breaker at
  `probes_left: 0`.

### 5. The new error — `HttpError::CircuitOpen`

```rust
// net-api/src/error_kind.rs — new variant on the shared enum
pub enum ErrorKind { Timeout, Connection, Throttled, Auth, Client, Server, Unknown, CircuitOpen }

// net-http-api/src/error.rs
pub enum HttpError { /* … */ CircuitOpen }         // no source — a local decision
// impl HasErrorKind:
HttpError::CircuitOpen => ErrorKind::CircuitOpen
```

- **A genuinely new failure mode deserves a distinct kind.** A fast local reject is *not*
  a transport failure and *not* a throttle; mapping it to `Unknown` would make an open
  circuit observably indistinguishable from a real backend error in Telemetry, and
  `Throttled` would conflate it with `RateLimit`'s proactive wait. So both a new
  `HttpError::CircuitOpen` **and** a new `ErrorKind::CircuitOpen` are added.
- **Non-retryable by construction.** `Retry::is_transient` is `{Timeout, Connection}`, so
  `CircuitOpen` is never retried even if it *were* seen — and it is not: the breaker sits
  **outside `Retry`**, so `CircuitOpen` short-circuits above the retry loop and surfaces
  straight to `Tracing`/the adapter. Nothing above the breaker retries.
- **Touch points:** the `net-api` `ErrorKind` enum + its exhaustive `kind()`-mapping test
  and any exhaustive `match` in that crate; `net-http-api`'s `error.rs` `HasErrorKind`
  impl + its `kind_maps_each_variant` test gain the `CircuitOpen` row.

### 6. Stack interaction (ADR-0031 §1–§2, §5)

`Tracing → CircuitBreaker → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf`.
`CircuitBreaker` is **outside `Retry`**, so it counts the **logical, post-retry**
outcome — a request that `Retry` re-issued three times and still failed is **one**
`Failure` to the breaker, not three — and an Open circuit short-circuits **before**
`Retry`/`RateLimit`/`Timeout` run, spending none of their budget. It is body-transparent,
composing with `Retry`'s (and below it, `RateLimit`'s `Guarded<B>`) response type without
disturbing it — the breaker only reads `status()` and either forwards the response or
substitutes a local `CircuitOpen`.

## Testing (MockTimer-driven; pure-core table tests + Service integration)

The `Breaker` core is tested **synchronously** (no executor, `now` an input); the Service
is tested with `MockTimer::advance()` + an **inline** `Service` double that counts calls
and yields a scripted outcome sequence (no `MockClient` — cycle). `#[tokio::test]`
provides the executor.

**Pure `Breaker` table tests (zero async):**

- **Threshold trip.** `failure_threshold = 3`: two `Failure`s keep it `Closed`; the third
  consecutive `Failure` → `Open { now + cooldown }`.
- **Streak reset.** `Failure, Failure, Success, Failure` never trips (the `Success`
  resets the counter).
- **Ignored does not reset.** `Failure, Ignored, Failure, Failure` (threshold 3) **trips**
  — the `Ignored` left the streak intact (the anti-masking property).
- **Immediate throttle trip.** A single `TripNow` from `Closed` → `Open`, and the reopen
  instant is `now + throttle_cooldown` (asserted **distinct** from `cooldown`).
- **Open rejects then probes.** `Open` `admit`s `Reject` while `now < reopen_at`; at/after
  `reopen_at` the first `admit` returns `Pass` and transitions to `Half-Open`.
- **Half-Open close / re-open.** A probe `Success` (and, separately, an `Ignored`) →
  `Closed`; a probe `Failure` → `Open { now + cooldown }`; a probe `TripNow` → `Open {
  now + throttle_cooldown }`.
- **Concurrency gate.** With `half_open_probes = 1`, once the single probe is admitted a
  further `admit` returns `Reject` until the probe resolves; with `half_open_probes = 2`,
  two probes admit and both must resolve non-failing to close.
- **`classify` partition.** A table over `(Err kind | Ok status) → Class` covering every
  row of §3 (incl. `429 → TripNow`, `4xx → Ignored`, `Auth → Ignored`, `Unknown →
  Ignored`, `5xx → Failure`, `2xx → Success`).

**Service integration (`MockTimer` + inline leaf double):**

- **Trip then fast-reject.** `failure_threshold` consecutive `Connection` errors trip the
  circuit; the **next** call returns `Err(HttpError::CircuitOpen)` with the **leaf
  call-count frozen** (the request never reached the inner double).
- **Immediate 429 trip.** One leaf `Ok(429)` trips the circuit on the long cooldown; a
  subsequent call fast-rejects, and only advancing past `throttle_cooldown` (not the short
  `cooldown`) admits a probe.
- **Recovery.** After tripping, `advance` past `cooldown` → the next call is admitted as a
  probe (leaf hit **once**); a leaf `Ok(200)` **closes** (subsequent calls flow), whereas
  a leaf `Ok(503)`/`Connection` **re-opens** (next call fast-rejects again).
- **Shared state.** Two `CircuitBreaker` clones produced from one `CircuitBreakerLayer`
  observe the **same** trip (a failure streak driven through clone A opens the circuit
  seen by clone B) — proving the single-per-host `Arc` sharing.

## Dependencies

**No new dependency, no `net-http-api` `Cargo.toml` change.** `http`/`bytes`/`http-body`
are crate deps; `oath-adapter-net-mock` (`MockTimer`) + `tokio` are dev-deps — all
present since #76/#78. The breaker uses `std::sync::{Arc, Mutex}`,
`std::time::{Duration, Instant}`, `std::num::NonZeroU32`, and `net-api::Timer::now()`
only — **no** `futures-util` race, **no** `sleep`, **no**
`tokio`/`hyper`/`reqwest`/`serde`. The **`net-api`** crate gains one enum variant
(`ErrorKind::CircuitOpen`) — no new dependency there either.

## Definition of done

- `CircuitBreaker<S, T>` + `CircuitBreakerLayer<T>` + `CircuitBreakerConfig` + the pure
  `Breaker`/`BreakerState` core + the `classify` partition, implemented as specified in
  `circuit_breaker.rs`, with the tests above.
- `net-api` gains `ErrorKind::CircuitOpen`; `net-http-api` gains `HttpError::CircuitOpen`
  and its `HasErrorKind` row; both crates' exhaustive-mapping tests updated.
- `lib.rs` gains `pub mod circuit_breaker;` + re-exports + a module-doc bullet.
- ADR-0034 gains an **append-only** amendment (**#9** — the `Retry` layer takes #8; see
  Open questions) recording: the `CircuitBreaker` layer, the new `CircuitOpen`
  error/kind, the 4-class classification (`Connection`/`Timeout`/`5xx` → Failure;
  `Throttled`/429 → immediate TripNow on the long cooldown; `4xx`/`Auth`/`Unknown` →
  Ignored; `2xx`/`3xx` → Success), the `NonZeroU32` divergence from §5's `u32`,
  consecutive-count v1, single per-host, lazy Half-Open, and the deferred
  `Unknown → Failure` / rolling-window / per-key / state-watch.
- `just ci` green (fmt, lint = deny, test + doctests, doc, deny, typos, machete); no new
  warnings; no `unsafe`/`unwrap`/`expect`/indexing in non-test code (the `Mutex` poison
  is recovered, not unwrapped; there is no fallible arithmetic — counter increments are
  bounded by `failure_threshold` and a `saturating_add` guards the degenerate case).
- `CHANGELOG.md` `[Unreleased]` updated.
- Delivered as one issue → one branch (worktree `.claude/worktrees/net-http-circuit-breaker`)
  → one PR (`Closes #N`).

## Open questions (for the implementation plan)

1. **ADR amendment number.** On `main @ de2e5e4` ADR-0034's amendment list runs #1–#7;
   the `Retry` branch adds #8. If this branch is cut **after `Retry` merges** (or off the
   `Retry` branch), `CircuitBreaker` is **#9**. If it is cut off `main` **before `Retry`
   merges**, both would provisionally claim #8 — renumber to #9 on rebase (the same
   trail-completeness convention #78's Timeout spec and the `Retry` spec used). This spec
   assumes **#9**.
2. **`Instant` source in `MockTimer`.** Confirm `MockTimer::now()` returns an `Instant`
   that `advance()` moves forward monotonically, so the `reopen_at = now + cooldown`
   comparison and the "past `throttle_cooldown` but not `cooldown`" test are expressible
   (they are for `RateLimit`/`Timeout`/`Retry`, which already gate on `MockTimer` time).
3. **Test executor.** `#[tokio::test]` for the Service tests (parity with the shipped
   layers); the pure `Breaker` tests need no executor at all — a benefit of the
   pure-core cut.
4. **`half_open_probes > 1` semantics.** *Resolved:* v1 ships the `NonZeroU32` knob and
   the `probes_left`/`successes_needed` accounting handles `> 1`; IBKR uses `1`, but the
   `> 1` path **is table-tested now** (cheap, proves the generality) — see the Testing
   section's two-probe concurrency-gate case.
