# HTTP resilience and venue pacing: the layer stack, order-safe retry, keyed rate/concurrency limits, circuit breaker

> **Amended by [ADR-0034](0034-http-construction-surface-auth-guarded-boot-coverage.md):**
> §3's `Permit` enum is replaced by `Guarded<B>` carrying
> `Option<async_lock::SemaphoreGuardArc>`, released at the *earlier of* stream-end or
> drop; `RateLimitConfig<K>` must be total over `RateKey::all()` at construction.

[ADR-0030](0030-http-transport-contract-wire-bytes-streaming-composition.md) fixed the
HTTP transport contract (bytes in, streaming bytes out, `HttpClient`, hyper backend).
This ADR specifies the **middleware that wraps it** — the default layer stack and its
order, and the construction of each resilience layer — driven by the concrete pacing
rules of the first [Broker](../../CONTEXT.md), IBKR's Client Portal Web API. Every
timing layer is generic over
[`net-api::Timer`](0029-network-adapter-stack-transport-split-compile-time-composition.md);
the tokio impl lives in `net-http-hyper`.

## The grounding case — IBKR Client Portal pacing

IBKR enforces a **global 10 req/sec**, **per-endpoint** overrides, and a **429 → 15-minute
IP penalty box** (repeat violators permanently blocked). The per-endpoint column is not
uniform — it is rate *or* **concurrency**:

| Shape | Examples | Models as |
|---|---|---|
| `1/sec`, `1/5s`, `10/s`, `1/min`, `1/15min` | `/tickle`, `/iserver/account/orders`, `/iserver/marketdata/snapshot`, `/sso/validate`, `/iserver/scanner/params` | `TokenBucket { rate, burst:1.. }` |
| **5 concurrent requests** | `/iserver/marketdata/history` | `Concurrency { max: 5 }` |
| unlisted | everything else | global only |

This table drives §2–§4: a request counts against the global budget **and** its own
limit, that own limit is rate *xor* concurrency, and we must *never* hit a 429.

## Decision

### 1. The default stack and its order

```text
Tracing → CircuitBreaker → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf
```

(First `.layer()` is outermost — ADR-0029's `ServiceBuilder` invariant.) Rationale for
the order: `Tracing` spans the whole logical request (including retries and pacing
waits); `CircuitBreaker` short-circuits *before* `Retry` runs (§5); `RateLimit` is
*inside* `Retry` so each attempt spends budget; `Timeout` bounds the send, not the
permit wait; `BufferOrStream` is inside `Retry` so retry sees the buffered outcome
(ADR-0030 §4); `Auth` re-stamps current credentials per attempt. A trivial `SetHeaders`
(pure request-header stamp) folds in near `Auth`.

### 2. Order-safe retry — the wire layer never retransmits an order

A blind wire retry in front of `POST /order` duplicates the order on a timeout-then-retry
— a funded incident, not a bug. So `Retry` is **retryability-aware**:

- It decides per request from a `Retryability` request extension (`Copy`, survives
  replay), defaulting to **retry idempotent methods only** (`GET`/`HEAD`/`PUT`/`DELETE`);
  **never `POST`** unless explicitly marked.
- **Order retransmission is Core's job, not the transport's.** A timed-out order surfaces
  as a classified `HttpError`; Core decides whether to reconcile or re-issue under the
  *same* `Order Instruction Id`, per
  [ADR-0022](0022-reliable-order-path-graduated-failure.md) (graduated failure),
  [ADR-0006](0006-broker-reconciliation-contract.md) (reconciliation), and
  [ADR-0026](0026-order-identity-three-ids-deterministic.md) (the deterministic
  instruction id as the venue dedup key — the FIX `ClOrdID` role).
- It **never retries a `429`** (§5): retrying compounds IBKR's penalty box.

### 3. `RateLimit` — one keyed layer, rate *and* concurrency as policies

Two separate rate/concurrency layers cannot model an endpoint whose limit is concurrency
(`/iserver/marketdata/history`) while its neighbour's is rate — the per-endpoint policy
would split across two unsynced maps. So they are **one layer**, and concurrency is just
another policy:

```rust
enum LimitPolicy {                       // closed enum — NO dyn (extend by variant)
    TokenBucket { rate: f64, burst: u32 },   // every IBKR rate row, by parameters
    Concurrency { max: u32 },                // /iserver/marketdata/history = 5
}
enum Permit { Rate, Concurrency(OwnedSemaphorePermit) }   // acquire → guard
```

- **Buckets:** a `frozen Arc<HashMap<RateKey, Bucket>>` — the key set is known from config
  at construction, never mutated, so **lookup is lock-free and there is no map-wide lock**;
  each `Bucket` owns its **own `Mutex`** over its policy state, so contention is scoped to
  one endpoint. The lock is **released before any `await`** (compute deficit → unlock →
  `Timer::sleep` → retry), so a throttled request never blocks other acquirers of its
  bucket.
- **Per-request directive** (`http::Request` extension, replacing a classifier closure —
  the adapter knows the endpoint when it builds the request):

  ```rust
  struct RateLimit<K> { scope: Scope, key: Option<K> }
  enum Scope { None, Global, Local, Both }   // full 2×2 state space
  ```

  `None` → unlimited (acquire nothing — the *explicit* opt-out); `Global`/`Local`/`Both`
  → the obvious bucket sets; **absent directive defaults to `Global`** (you cannot bypass
  the global budget by forgetting to stamp). A `Global`/`Local`/`Both` request that
  references a bucket **missing from the map is a configuration error, not "no limit"**:
  the config is **validated at construction** (every key the adapter stamps must have a
  bucket), and any gap that still reaches runtime **fails closed** — the request is
  rejected as `Throttled`, never sent unthrottled. A silent fail-open path would bypass
  pacing straight into IBKR's 429 penalty box; only the explicit `None` is unlimited.
- **Acquire order:** rate-type buckets **before** concurrency-type, global-first — a
  request never holds a scarce concurrency permit while merely *waiting* on a rate token.
- **Permit lifetime:** a rate `Permit` is a ZST (acquire-and-go); a concurrency permit is
  **held for the request** and released at `call`-return for a **buffered** response (the
  real IBKR `/history` case — work done when the buffered fetch returns) or **attached to
  the response body** (released at stream-end/drop) for a **streaming** response. No caller
  discipline, no permit-handoff via extension.
- **Wait, with `max_wait`:** an exhausted bucket waits (backpressure, not failure) up to
  `max_wait`, then returns `Throttled`.

### 4. `LimitPolicy` is a closed enum — no `dyn`

Both `net-http-api` and adapters are first-party, so a new venue's limit shape is a new
enum variant we add when we meet it (YAGNI), dispatched by `match` — no vtable, no alloc.
A `Custom(Arc<dyn …>)` escape hatch was explicitly rejected for reintroducing the dynamic
dispatch the whole stack avoids (ADR-0029 §5). `FixedWindow` is *not* added now — IBKR
needs only `TokenBucket` + `Concurrency`.

### 5. `CircuitBreaker` — the 429 backstop

`RateLimit` is the **proactive** guard (never hit 429); `CircuitBreaker` is the
**reactive** one (stop cold if we do). Three states, `Timer`-driven:

```rust
struct CircuitBreakerConfig {
    failure_threshold: u32,       // consecutive Connection/Server/Timeout → Open
    cooldown: Duration,           // general outage, e.g. 30s
    throttle_cooldown: Duration,  // penalty box ≈ 15 min, on Throttled/429
    half_open_probes: u32,        // 1
}
```

- **Closed → Open** on `failure_threshold` consecutive `Connection`/`Server`/`Timeout`
  (consecutive-count for v1; rolling-window later), **or immediately on `Throttled`/429**
  with the long `throttle_cooldown` (IBKR's 15-min box).
- **Open** rejects fast with a **non-retryable `CircuitOpen`** `HttpError` — and it sits
  **outside `Retry`**, so it counts *logical* (post-retry) outcomes and short-circuits
  before `Retry`/`RateLimit` run.
- **Half-Open** after cooldown admits `half_open_probes`; success closes, failure re-opens.
- **Single per-host breaker** (v1): IBKR's penalty box is **per-IP, venue-wide**, so one
  breaker for the whole gateway matches reality. State shared behind `Arc`.

### 6. `Tracing` — a Telemetry source, secret-safe

`TracingLayer` (in `net-http-api`, on the zero-runtime `tracing` facade) is **outermost**:
one span per logical request covering retries and pacing waits, with `Retry` emitting
per-attempt events within it. It records method / route / status / `ErrorKind` / latency /
attempt count and **never** logs auth material or bodies (no `Authorization`/`Cookie`/API
keys/query tokens) — the Auth layer injects secrets, so Tracing is the one place certain
not to leak them. Its output is the
[ADR-0014](0014-observability-three-planes-deterministic-boundary.md) **Telemetry** plane:
the net stack runs in the Adapter process, outside Core's deterministic fold, so this is
machinery metrics (latency/throughput), never canonical state. The layer only instruments;
aggregation is a subscriber's job. It is **always-on but pay-per-use** — an omitted
`TracingLayer` is zero code in a hand-rolled stack; only `build()`'s default includes it.

## Considered options

- *Two layers (rate + concurrency)* — rejected: cannot place an endpoint's rate-xor-concurrency
  limit in one synced acquire pass; the IBKR table forces the merge.
- *Concurrency permit handed to the caller via response extension* — rejected: makes release
  depend on caller discipline and breaks when the response is destructured; body-attachment
  releases automatically.
- *`Custom(Arc<dyn LimitPolicy>)`* — rejected: smuggles `dyn` back in; the closed enum is the
  first-party extensibility mechanism.
- *Map-wide lock / per-clone buckets* — rejected: a map lock serialises all endpoints; a
  per-clone bucket silently multiplies the real rate. Frozen `Arc<HashMap>` + per-bucket
  `Mutex` is the only correct shape.
- *`CircuitBreaker` inside `Retry`* — rejected: `Retry` would retry the open-circuit
  rejections, defeating it; the breaker must wrap `Retry`.
- *Retry keyed purely on `ErrorKind`* — rejected: it duplicates orders on `POST` retry and
  hammers the 429 penalty box; safety must be structural (retryability-aware + no-retry-429).

## Consequences

- `RateLimit`, `CircuitBreaker`, `Timeout`, `Retry` are generic over `net-api::Timer`;
  `RateLimit`/`CircuitBreaker` use `Timer::now()` (ADR-0029 §4 added it for exactly this).
  All are mockable with a fake clock.
- The IBKR adapter supplies the `RateLimit` config map + `RateKey` type from the pacing
  table, stamps `RateLimit`/`Retryability`/`BufferMode` extensions when building each
  request, owns the effectful `tickle` keepalive as a wrapping layer, and provides the
  `CircuitBreaker`/`Retry`/`Timeout` configs.
- `ConcurrencyLimit` as a *separate* layer is dropped — concurrency is a `LimitPolicy`
  inside `RateLimit`.

## Relationships

Wraps **ADR-0030** (the HTTP contract) and rests on **ADR-0029** (`Timer`, composition,
no `dyn`). Defers order retransmission to **ADR-0022 / 0006 / 0026**. Routes net-layer
observability to the **ADR-0014** Telemetry plane. Glossary unchanged — implementation
vocabulary only; IBKR pacing values are reference data for the adapter, not domain terms.

## Amendments (2026-07-06)

Recorded append-only (the decision text above is unedited).

1. **`classify` distinguishes a `Throttled` *error* from a `429` *response* (C1 fix).**
   §5 wrote the trip trigger as "immediately on `Throttled`/429", conflating two different
   things. `HttpError::Throttled` is produced **only locally** by `RateLimit` — a `max_wait`
   breach, an absent `RateScope`, or a missing bucket, where the request is *never sent* — so
   it carries no host-health signal. Because `CircuitBreaker` sits **outside** `RateLimit`,
   mapping that error to a trip let a single local pacing rejection open the venue-wide
   breaker for the full `throttle_cooldown` (the ~15-minute penalty box): a self-inflicted
   outage. **Clarified:** only a venue **`429` *response*** (`Ok(Response)` with status 429)
   trips the breaker (`Class::TripNow`); a `Throttled` *error* is `Class::Ignored` (never
   trips, and never resets the Closed-state streak). The proactive-limiter (`RateLimit`) /
   reactive-breaker (`CircuitBreaker`) split of §5 is otherwise unchanged. See the
   [deep-review](../superpowers/plans/2026-07-06-net-http-deep-review.md) finding C1.
