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

6. **`Timeout` layer (Slice 1 PR 2).** The `Timeout<S, T>` layer + `TimeoutLayer<T>`
   factory bound the **send** (`inner.call` → response), not the pacing-permit wait
   (ADR-0031 §1) — a response-future race against `Timer::sleep`, `HttpError::Timeout`
   on the deadline (inner future dropped). Body-transparent: `http::Response<B>` is
   returned untouched (no `Guarded`-style carrier, no `B: Body` bound). A per-request
   `RequestTimeout(Duration)` extension overrides the layer default; an **absent**
   extension uses the default (not fail-closed, unlike `RateScope` — a missing override
   has no fail-open pacing hazard, the global deadline still applies). A `TimeoutBody`
   bounding a *streaming* transfer's mid-stream stall is **deferred**: it is inert on
   IBKR's all-buffered responses (a `Buffered` body is already in memory when `call`
   returns) and lands additively when a streaming venue first needs it.

7. **`AuthSource` is two same-shaped per-transport traits, not one "identical",
   `Parts`-based trait (corrects 0032 §8, confirms §1's shipped shape).** ADR-0032 §8
   and its Consequences describe `AuthSource` as an "identical one-method trait" that
   "operates on `http::request::Parts` … so the same shape serves HTTP's
   `Request<Bytes>` and the WS `Request<()>`." That overclaims on three axes, and the
   shipped HTTP trait (PR #66, `crates/adapter/net/http/api/src/auth.rs`) already
   abandoned `Parts` — correctly:
   - **Whole request, not `Parts`.** Some HTTP schemes **sign the request body**
     (Binance signed REST HMAC-SHA256s the payload — the Binance/Coinbase generality
     ADR-0033 deliberately cross-checks against). A `Parts`-only `authorize`
     structurally cannot see the body to sign it, so HTTP takes the whole
     `&mut http::Request<Bytes>`. The WS *upgrade* is a bodyless GET, so WS takes
     `&mut http::Request<()>` — body-agnostic **there** because there is no body, not
     because auth is universally body-agnostic. So `Parts` is under-general, not a
     unifier.
   - **Different error type — so not "identical".** HTTP `authorize` returns
     `Result<(), HttpError>`; the WS trait returns `Result<(), WsError>` (each
     transport's own error). "Identical" cannot hold across two body types **and** two
     error types.
   - **Resolution:** two same-*shaped* per-transport traits — `net-http-api`:
     `authorize(&mut http::Request<Bytes>) -> Result<(), HttpError>` (shipped, §1
     unchanged); `net-ws-api`: `authorize(&mut http::Request<()>) -> Result<(), WsError>`
     — each `Clone + Send + Sync`, re-stamped per attempt (HTTP) / per (re)connect (WS,
     0032 §8). IBKR's single `IbkrAuthSource` (header/cookie, body-agnostic) impls both;
     a body-signing venue impls only the HTTP one. **Rejected:** the `Parts` unification
     (under-general — cannot body-sign) and a generic shared `AuthSource<B>` (reintroduces
     the shared `net-auth-api` crate 0032 §8 itself rejected). Lands with the WS
     `AuthSource` declaration in the WS auth slice; the HTTP trait needs no change.

8. **`Retry` layer (Slice 1 PR 3).** The `Retry<S, T>` layer + `RetryLayer<T>`
   factory re-issue an **explicitly-eligible** request — a `Retryable` marker
   extension; **absent → never retried**, tightening §2's idempotent-*method*
   default into fail-safe adapter-stamped opt-in (the same structural-safety move
   Amendment #1 made for `RateScope`; a forgotten stamp never duplicates a `POST`)
   — on a **transient** failure (`HttpError::{Timeout, Connection}`) or a `5xx`
   response, with capped-exponential **full-jitter** backoff
   (`delay ∈ [0, min(cap, base·2ⁿ⁻¹)]`) up to `RetryConfig::max_attempts`. A **429**
   / other 4xx, an `Auth`/`Throttled` error, or an `Other` error is **never**
   retried (ADR-0031 §2/§5); on exhaustion the **last** outcome is returned
   verbatim (no synthesized error). Body-transparent — it drops a superseded
   response, releasing that response's `Guarded` permit. Jitter uses an internal
   seeded `SplitMix64` (no `rand` dependency, no injected `Jitter` generic — the
   RNG is a pure computation); a **total-elapsed retry budget** and **`Retry-After`
   parsing** are deferred (each an additive follow-up). No new dependency.

9. **`CircuitBreaker` layer (Slice 1 PR 4).** The `CircuitBreaker<S, T>` layer +
   `CircuitBreakerLayer<T>` factory add the **reactive** backstop to `RateLimit`'s
   proactive pacing (ADR-0031 §5). A pure, clock-injected `Breaker` state machine
   (Closed/Open/Half-Open) — table-tested with zero async — sits behind a thin
   `Arc<Mutex<Breaker>>` + `Timer` Service shell. It trips **Open** on
   `CircuitBreakerConfig::failure_threshold` consecutive `Connection`/`Timeout`/`5xx`
   failures, or **immediately** on a `Throttled`/429 with the long `throttle_cooldown`
   (IBKR's penalty box); while Open it **fast-rejects** with a **new non-retryable
   `HttpError::CircuitOpen` / `ErrorKind::CircuitOpen`** without touching the inner
   stack; after the cooldown it admits `half_open_probes` **Half-Open** probes (a
   reached-host outcome closes, a failure re-opens). Outcomes are a **4-class
   partition**: `Connection`/`Timeout`/`5xx` → *Failure*; `Throttled`/429 →
   *TripNow*; `4xx`/`Auth`/`Unknown` → *Ignored* (never trips, and **never resets the
   Closed-state failure streak** — so an interleave cannot mask a building outage; an
   `Auth` error must not trip the gateway; in **Half-Open** a reached-host `Ignored`
   still resolves the probe like a `Success`); `2xx`/`3xx` → *Success*. `failure_threshold`/`half_open_probes` are
   `NonZeroU32` (typing §5's `u32` — "≥ 1" a type invariant, infallible `new`). A
   **single per-host** breaker shared behind `Arc`; **consecutive-count** for v1;
   `now()`-only timing (lazy Open→Half-Open, no sleep, no `futures-util`, no new
   dependency). It sits **outside `Retry`**, counting logical post-retry outcomes.
   Deferred: the resilience4j fail-safe `Unknown → Failure`, rolling-window counting,
   per-key breakers, and a breaker-state observation watch.
10. **`Tracing` layer (Slice 1 PR 5).** The outermost `Tracing<S, T>` layer +
   `TracingLayer<T>` factory (ADR-0031 §6) open one `info` span per logical request
   and attach it to the inner future via `tracing::Instrument`, so downstream events
   — including `Retry`'s per-attempt events — nest under it. The span records method,
   `route` (`uri().path()` — the **query is dropped**, since it can carry tokens),
   `status` **xor** `ErrorKind` (a `_`-arm label over the `#[non_exhaustive]` enum),
   `latency_us` (via `Timer::now()` deltas — the layer is `Timer`-generic), and
   `attempts`. Routed to the ADR-0014 Telemetry plane (machinery metrics, lossy, never
   canonical). **Secret-safe by construction:** the layer reads only method, path,
   status, `ErrorKind`, and the clock — never headers, never the body. Body-transparent
   (`http::Response<B>` untouched, no `B: Send` bound — nothing of type `B` crosses the
   single await). **Composition contract:** `Tracing` owns the one per-request span;
   inner resilience layers emit `tracing` **events**, never open their own span — which
   keeps `Span::current()` at any inner depth resolved to `http.request`, so `Retry`'s
   `Span::current().record("attempts", n)` populates the field (a graceful no-op when no
   such span/field is active). Adds the `tracing` facade (runtime dep, zero executor) +
   `tracing-subscriber` (dev-dep). The module is named `trace` to avoid shadowing the
   `tracing` crate; the public types are `Tracing`/`TracingLayer`.
11. **`stack()` assembly + `HttpConfig` (Slice 2, assembly).** `stack<S, T, A, K>()`
    (`net-http-api`) assembles the canonical resilience order (ADR-0031 §1) over an
    arbitrary leaf: `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders(
    Auth( leaf ) ) ) ) ) ) )`. It builds the one fallible layer (`RateLimitLayer::new`,
    which runs `validate_coverage` + `validate_concurrency_singleton`) **first**, so a
    coverage/param/singleton failure is a `BuildError` before the infallible layers are
    assembled — `stack()` does **not** call `validate_coverage` separately. `Auth`/
    `SetHeaders` are direct `Service` wrappers (no `Layer` factory), so they pre-wrap
    the leaf; the five `Layer`-factory layers compose over that via the kernel's
    `ServiceBuilder`. The return bound is the full `impl HttpClient + Clone + Send +
    Sync + 'static` (not bare `impl HttpClient`), so a `Send`/`Clone`/`'static`
    regression in any layer is a compile error *at `stack()`*; `build()` (the following
    hyper-backend slice) reuses this bound over the hyper leaf. `HttpConfig` is
    non-generic plain data — `timeout`, `retry`, `circuit_breaker`, `headers`, and
    `rate_limit_max_wait` (the permit-wait ceiling feeding `RateLimitLayer::new`,
    distinct from the send `timeout` because `RateLimit` sits outside `Timeout`) — with
    no type parameter and no `serde` (deserialisation stays in the adapter, ADR-0003).
    The one generic pacing arg (`RateLimitConfig<K>`), `auth`, and `timer` are separate
    `stack()` parameters. **Bound refinement:** the spec sketch's `T: Timer, A:
    AuthSource, K: RateKey` becomes `T: Timer + 'static, A: AuthSource + 'static, K:
    RateKey + Debug` in the implementation, plus the leaf's `S::Body: Send`
    (transitively required by `Retry`/`RateLimit`'s existing `B: Send` bound) — the
    composed value is returned `'static`; coverage validation renders the offending
    key. `BufferOrStream` is **not** a
    layer here — buffering is a leaf-side body-construction concern, so the innermost
    leaf already satisfies "inside `Retry`". Full-stack ordering invariants are
    regression-tested over an inline recording leaf + `MockTimer` (not `MockClient`,
    which would close the net-http-mock → net-http-api dev-dep cycle and cannot script
    sequences). No new dependency; no existing-layer change.
