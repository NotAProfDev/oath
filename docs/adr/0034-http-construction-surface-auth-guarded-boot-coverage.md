# HTTP construction surface: the AuthSource seam, the permit-carrying Guarded body, boot-time pacing coverage

[ADR-0030](0030-http-transport-contract-wire-bytes-streaming-composition.md) and
[ADR-0031](0031-http-resilience-venue-pacing.md) fixed the HTTP transport contract and
its resilience stack, but left three construction seams underspecified — each a type or
trait decision an implementer must make before `net-http-api` compiles: the `AuthSource`
referenced but never defined, the concurrency permit's lifetime (named a `tokio` type
inside a crate 0030 declares `tokio`-free — a contradiction), and the `build()` generic
surface. This ADR records the decisions closing them, per the approved
[construction-surface spec](../superpowers/specs/2026-06-30-net-http-construction-surface-design.md),
and amends 0030/0031 **append-only** (the landed texts are not edited; each gains a
pointer to this ADR).

## Decision

### 1. `AuthSource` — the credential seam (defines what 0030 referenced)

`net-http-api` defines the seam the adapter implements; the `Auth` layer calls it
**innermost** — inside `Retry`, once per attempt, against the final buffered request —
which is what makes per-attempt re-signing (fresh HMAC timestamp/nonce) and
current-token stamping correct:

```rust
pub trait AuthSource: Clone + Send + Sync {
    fn authorize(&self, req: &mut http::Request<Bytes>)
        -> impl Future<Output = Result<(), HttpError>> + Send;
}
```

- **`fn -> impl Future + Send`, not `async fn`** — the composed stack future must be
  `Send`; only the desugared form can require it (matches `Timer::sleep`, 0029 §4, and
  `HttpClient::send`, 0030 §6).
- **Mutates `&mut http::Request<Bytes>` in place** — covers static bearer, HMAC over the
  buffered body, async OAuth refresh, and cookie/no-op, with no per-call `HeaderMap`
  allocation.
- **Errors are `HttpError`** via the `Auth(String)` variant (added in PR 2 for this seam) (→ `ErrorKind::Auth`) —
  both types live in `net-http-api`; no `From`/`map_err` shim.
- **`NoAuth`** (ready `Ok(())`) is the IBKR impl — the local gateway holds the session
  cookie.
- **Static headers sit just outside `Auth`** (a `SetHeaders` layer), so dynamic
  credentials win any key collision — `Auth` is the last writer before the leaf.

### 2. `Guarded<B>` — the concurrency permit rides the body (amends 0031 §3)

0031 §3 attached a `Permit` enum holding `tokio::sync::OwnedSemaphorePermit` "to the
response body" with nowhere to carry it. Replaced by:

```rust
pub struct Guarded<B> { inner: B, permit: Option<async_lock::SemaphoreGuardArc> }
```

- **`RateLimit` always returns `http::Response<Guarded<B>>`** — one static type.
  `permit: None` for rate-scoped, unscoped, or buffered responses; `permit: Some(_)`
  only for a streaming concurrency-scoped response (IBKR `/history`). The `Rate` enum
  arm was redundant with `None`.
- **Released at the *earlier of* stream-end or drop** — `poll_frame` `take()`s the
  permit on the terminal frame (a fully-read but still-held body must not waste one of
  `/history`'s 5 permits); the field's `Drop` covers early abort.
- **Body-attach, not response-future-attach** — tower's stock `ConcurrencyLimit` frees
  the permit at headers, under-counting concurrency through a streaming transfer. This
  is the hyper connection-pool model (`Pooled<T>`); do not "simplify" it back.
- **Wrapper transparency** — `Guarded` (and 0030 §3's `ResponseBody<B>`) must forward
  `is_end_stream` and `size_hint`, not just `poll_frame`: the http-body 1.x defaults
  (`false`/unbounded) silently break `collect()` pre-sizing and make any
  `size_hint().upper()` max-size guard fail open.
- **The semaphore is `async-lock`, not `tokio::sync`** — resolves the 0030/0031
  contradiction. 0030's charter is amended from "free of `tokio`/`hyper`/`reqwest`/
  `serde`" to **"free of any async *runtime* — and of `hyper`/`reqwest`/`serde`"**;
  `net-http-api`'s graph names no runtime. (Both candidate semaphores are
  reactor-free; `async-lock` is chosen so the graph *states* neutrality. Honest cost:
  two small smol-rs crates vs zero new crates for `tokio` `features=["sync"]` —
  a thin margin, decided on ADR-0029's neutrality charter.)

### 3. `stack()`/`build()` and boot-time total pacing coverage (amends 0031 §3)

The canonical layer assembly lives once in `net-http-api` as
`stack(leaf, cfg, timer, auth, rate_limits)` over an arbitrary leaf;
`net-http-hyper::build(...)` constructs the hyper leaf and delegates. The return bound
is `impl HttpClient + Clone + Send + Sync + 'static` (a regression in any layer becomes
a compile error at `stack()`). The split exists so the **ordering invariants**
(CircuitBreaker outside Retry; RateLimit inside Retry) are regression-testable over
`stack(MockClient, …, MockTimer)`.

`RateKey` is a typed enum with a finite universe (`fn all() -> &'static [Self]`), and
`RateLimitConfig<K>` is a **total** map (`LimitDecl::{Policy, GlobalOnly}` — explicit
classification, not "absent"): `stack()`/`build()` return
`Err(BuildError::UndeclaredKey)` for any unclassified endpoint, so a missing pacing
bucket is a **boot failure**, not a first-live-order 429 → 15-minute IBKR penalty box.
0031 §3's runtime `Throttled` fail-closed is demoted to an unreachable backstop.

### 4. HTTP error statuses are not error-ified (clarifies 0030 §5)

`HttpError` carries **transport/middleware failures only**. HTTP 4xx/5xx
*statuses* are not converted to errors: they flow through as
`Ok(http::Response)` with the body intact, so the adapter reads the venue's
rejection payload and the stack never discards it (0030 §5, whose `HttpError`
examples were always middleware failures — timeout, retry-exhausted, body-read —
never statuses). The resilience layers decide by **peeking** `Response::status()`
(5xx → server-error signal) and the 429 `Retry-After` header — read-only; the
response continues downstream unchanged.

## Consequences

- `net-http-api` gains `async-lock` (+ `event-listener`) and stays runtime-free; the
  `HttpError::Auth` variant and the `Guarded<B>` type are part of the public contract.
- Adapters implement `AuthSource` once per venue; `NoAuth` ships in `net-http-api`.
- Implementation lands in slices: `AuthSource`/`Auth`/`SetHeaders`/`Guarded` first;
  `RateKey`/`RateLimitConfig`/`BuildError` next; the resilience layers and
  `stack()`/`build()` + the hyper leaf in later slices.

## Amendments (2026-07-04)

Refinements from a follow-up design review, recorded append-only (the decision text
above is not edited). Each lands with its implementation slice; the
[construction-surface spec](../superpowers/specs/2026-06-30-net-http-construction-surface-design.md)
carries the full reasoning.

1. **Absent `RateLimit<K>` directive fails closed (tightens §3 / 0031 §3).** With
   config-side totality (§3) in place, 0031's "absent directive defaults to `Global`"
   was the one remaining *silent* under-pacing path: an adapter that adds an endpoint
   and forgets to stamp the directive would fly global-paced only, invisible to the
   boot check because the gap is in the request, not the config. So a request with
   **no** `RateLimit<K>` extension is now **rejected fail-closed** (the same
   non-retryable classification error as the missing-bucket backstop). "Global only"
   is said with an explicit `Scope::Global`; opt-out with `Scope::None`. Lands with
   the `RateLimit` slice.

2. **`Guarded` releases the permit on a mid-stream error too (refines §2).** §2's
   eager release fires on the clean terminal frame (`None`); it now *also* fires on a
   mid-stream `Some(Err(_))` — a connection reset during a `/history` transfer is as
   over as a clean end (http-body 1.x yields no further frames after an error), and a
   consumer holding the errored body for error context must not pin one of `/history`'s
   5 permits. Release rule: **the earliest of terminal frame, mid-stream error, or
   drop.** Touches the `Guarded` body shipped in PR #66; lands as a follow-up code
   change.

3. **The `async-lock` choice rests on the stated multi-backend goal (sharpens §2's
   rationale).** OATH intends non-tokio backends (smol/async-std) — a stated goal, not
   a hedge — which decides `async-lock` (option R) over `tokio` `features=["sync"]`
   (option P) on a concrete basis, not a "thin margin": under P a future *smol* stack
   would drag `tokio` into its graph purely for the semaphore; under R the whole smol
   stack is genuinely `tokio`-free. Option Q (a `Timer`-style semaphore trait) is
   unnecessary, not merely YAGNI — `async_lock::Semaphore` is already cross-runtime, so
   backends reuse the same layer semaphore; nothing to abstract.

4. **`MockTimer` is *relocated* into its own dev-only `oath-adapter-net-mock` crate**
   (`crates/adapter/net/mock`), beside the `Timer` contract in `net-api` — *not* left
   in `net-http-mock`, which keeps only `MockClient`. `MockTimer` already ships in
   `net-http-mock` (`crates/adapter/net/http/mock/src/timer.rs`, PR #66); this moves
   it and re-points the http-stack tests. **Time-critical:** the WS resilience slice
   (ADR-0033 §9) is imminent and `net-ws-mock`'s own header already says
   "`MockTimer`/`MockSpawn` arrive with the resilience slice" — without the
   relocation that slice must either duplicate `MockTimer` or dev-depend a *WS* mock
   on an *HTTP* mock (the nonsense edge across the crate cut). Extracting to a shared
   `net-mock` lets both stacks share one fake clock. Both mocks keep the
   production-reachability guard (`cargo tree -e no-dev -i …` → no non-dev dependents).

5. **`RateLimit` layer (Slice 1 PR 1).** `LimitPolicy::TokenBucket` gains a
   `per: Duration` so IBKR's sub-1/second limits (`1/5s`, `1/min`, `1/15min`) are
   expressible with integer parameters; `validate_coverage` rejects a zero period.
   The per-request directive ships as `RateScope<K>` (renamed from §3's
   `RateLimit<K>` sketch, which collided with the layer name). The
   ≤1-concurrency-permit invariant (`Guarded` holds one) is enforced at
   construction by `BuildError::MultipleConcurrency` / `validate_concurrency_singleton`.
