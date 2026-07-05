# net-http `stack()` assembly + `HttpConfig` — design

**Status:** Approved design, pre-implementation.
**Date:** 2026-07-05.
**Crate:** `oath-adapter-net-http-api` (`crates/adapter/net/http/api`).
**Slice:** Slice 2 (assembly), runtime-free half. The hyper leaf + `build()` +
`TokioTimer` are a separate, following slice (the "hyper-backend slice").

## Context

The [net-http construction-surface spec](2026-06-30-net-http-construction-surface-design.md)
(Seam #3) fixes the `stack()`/`build()` split, the return bound, the config split,
and boot-time pacing coverage. [ADR-0031 §1](../../adr/0031-http-resilience-venue-pacing.md)
fixes the canonical layer order. This spec closes the last runtime-free gap in that
surface: the concrete `stack()` assembly and the `HttpConfig` aggregate that feeds it.

Everything the assembly composes already ships in `oath-adapter-net-http-api`:

- All five layers — `Tracing` (#86), `CircuitBreaker` (#85), `Retry` (#82),
  `Timeout` (#78), `RateLimit` (#76) — plus `Auth`/`SetHeaders`/`Guarded` (#66).
- The `RateScope<K>`/`Scope` per-request extension with **absent ⇒ fail-closed**
  (`HttpError::Throttled`, non-retryable) shipped *with* `RateLimit` (#76,
  ADR-0034 Amendment #1).
- `RateKey`, `RateLimitConfig<K>`, `LimitPolicy`/`LimitDecl`, `BuildError`, and the
  standalone `validate_coverage` boot check (#72).

So this slice adds **no new dependency**, no runtime, and **changes no existing
layer** — it is pure composition plus the config aggregate, plus the full-stack
ordering-invariant tests that only an assembly makes possible.

### Governing decisions (inherited, not re-litigated)

- **Layer order** — [ADR-0031 §1](../../adr/0031-http-resilience-venue-pacing.md):
  `Tracing → CircuitBreaker → Retry → RateLimit → Timeout → BufferOrStream → Auth → leaf`
  (first `.layer()` outermost, ADR-0029). `SetHeaders` folds in just outside `Auth`.
- **`stack()` signature + return bound** — construction-surface spec, Seam #3.
- **Config split** — non-generic `HttpConfig` data + the single `K`-generic
  `RateLimitConfig<K>` arg; `serde` stays in the adapter (ADR-0003).
- **Boot-time coverage** — `stack()` calls `validate_coverage` before assembling, so a
  missing/ill-configured bucket is a `BuildError`, not a first-live-order 429.

## Goal

Deliver `HttpConfig` and `stack()` so the canonical resilience stack can be assembled
once, over any leaf, and its **ordering invariants** (not just per-layer behaviours)
are regression-tested deterministically over an inline recording leaf + `MockTimer`.

## Scope (in)

- **`HttpConfig`** — a non-generic aggregate of the per-layer configs + static headers.
- **`stack<S, T, A, K>()`** — validate-then-compose the canonical order over an
  arbitrary `HttpClient` leaf, returning `Result<impl HttpClient + Clone + Send + Sync
  + 'static, BuildError>`.
- **Ordering-invariant + boot-coverage + fail-closed tests** over
  `stack(<inline recording leaf>, …, MockTimer)` (no `MockClient` — see Testing).
- **ADR-0034 amendment + CHANGELOG.**

## Non-goals (deferred — the hyper-backend slice)

| Deferred item | Why | Lands with |
| --- | --- | --- |
| `build()`, `hyper_leaf(conn)`, `ConnConfig` | Backend-specific; `build() = stack(hyper_leaf(conn), …)` over the same assembly | hyper-backend slice |
| `TokioTimer`, rustls/HTTPS connector, `hyper::Error → HttpError` | Runtime/TLS integration behind the `HttpClient` seam | hyper-backend slice |
| `BufferOrStream` as a middleware | Not a layer — buffering is a leaf-side body-construction concern (`ResponseBody::buffered`/`streaming` per `BufferMode`); the innermost leaf already satisfies "inside `Retry`" | hyper-backend slice (leaf) |
| `serde` on `HttpConfig` | Config deserialisation is an adapter concern (ADR-0003) | IBKR adapter slice |

## Decisions

### `HttpConfig` — one field per configurable layer

```rust
/// Non-generic assembly configuration: one field per configurable layer, plus the
/// static default headers. The `K`-generic pacing map (`RateLimitConfig<K>`),
/// `auth`, and `timer` are separate `stack()` args, so this type carries no type
/// parameter and no `serde` (deserialisation stays in the adapter, ADR-0003).
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Per-attempt send timeout (bounds the send, not the permit wait).
    pub timeout: Duration,
    /// Retry policy (attempts, backoff schedule).
    pub retry: RetryConfig,
    /// Circuit-breaker thresholds and cooldown.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Static default request headers, stamped by `SetHeaders` (just outside `Auth`).
    pub headers: HeaderMap,
    /// Ceiling on how long an exhausted bucket back-pressures before the request
    /// returns `HttpError::Throttled` — `RateLimitLayer::new`'s third parameter.
    /// Distinct from `timeout`: `RateLimit` sits **outside** `Timeout`, so the permit
    /// wait is bounded by this, not the send timeout — and at IBKR's 1/15-min buckets
    /// it is minutes, not seconds.
    pub rate_limit_max_wait: Duration,
}
```

**Five fields, exactly.** `Tracing` needs no config (only the clock). The pacing
*map* is the one generic arg (`RateLimitConfig<K>`), isolated so `HttpConfig` stays
non-generic; the pacing *permit-wait ceiling* (`rate_limit_max_wait`) is non-generic
and lives here, feeding `RateLimitLayer::new`'s third parameter. `Auth` is supplied as
the `auth: A` value. `HttpConfig` is a plain struct literal (not `#[non_exhaustive]`):
adapters construct it directly, matching every other config type in the crate; a
future field is a deliberate, reviewed breaking change, not something to pre-absorb
(YAGNI).

### `stack()` — validate, then compose in canonical order

```rust
/// Assemble the canonical resilience stack (ADR-0031 §1) over an arbitrary leaf.
///
/// Builds the fallible `RateLimit` layer **first**, so a config that is not total
/// over `K::all()` (or carries an out-of-range policy param, or breaches the
/// ≤1-concurrency-permit invariant) is a `BuildError` before the infallible layers
/// are assembled. Then composes, outermost-first:
/// `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders( Auth( leaf ) ) ) ) ) ) )`.
///
/// # Errors
/// [`BuildError`] if `rate_limits` is not total over `K::all()`, any policy is out of
/// range, or the concurrency-singleton invariant is breached — all propagated from
/// `RateLimitLayer::new` (which runs `validate_coverage` +
/// `validate_concurrency_singleton` internally).
pub fn stack<S, T, A, K>(
    leaf: S,
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where
    S: HttpClient + Clone + Send + Sync + 'static,
    S::Body: Send,
    T: Timer + 'static,
    A: AuthSource + 'static,
    K: RateKey + fmt::Debug,
```

> **Bound refinements over the construction-surface sketch.** The sketch wrote
> `T: Timer, A: AuthSource, K: RateKey`. The implementation adds: `S::Body: Send`
> (transitively required — `Retry`/`RateLimit` carry a `B: Send` bound, and
> `HttpClient::Body` is not `Send` by default, so it must surface at `stack()`);
> `T`/`A` `+ 'static` (the composed value is returned `'static`); and `K: + Debug`
> (`RateLimitLayer::new` renders the offending key when coverage fails). Recorded in
> ADR-0034 Amendment #11.

- **Validate via the RateLimit layer.** `RateLimitLayer::new(&rate_limits, timer,
  cfg.rate_limit_max_wait)?` runs `validate_coverage` + `validate_concurrency_singleton`
  and is constructed first, so a coverage/param/singleton failure short-circuits with
  `BuildError` before the rest is assembled (fail-closed at construction). `stack()`
  does not call `validate_coverage` separately — that would double-validate.
- **One clock, cloned in.** The single `timer: T` is cloned into each timing layer
  (`CircuitBreaker`, `Retry`, `RateLimit`, `Timeout`, `Tracing`); `Timer: Clone`.
- **Assembly mechanism.** `Auth`/`SetHeaders` are direct `Service` wrappers
  (`::new(inner, …)`, no `Layer` factory), so they pre-wrap the leaf; the five
  `Layer`-factory layers compose over that via the kernel's `ServiceBuilder` (first
  `.layer()` = outermost). The composed value auto-satisfies `HttpClient` (blanket
  impl). The mechanism is internal; the observable contract is the order + return bound.
- **Return bound.** The full `impl HttpClient + Clone + Send + Sync + 'static` (not
  bare `impl HttpClient`) turns any `Send`/`Clone`/`'static` regression in a layer
  into a compile error *at `stack()`*, and promises adapters the share/spawn they
  need. `build()` (next slice) reuses this exact bound over the hyper leaf.
- **`BufferOrStream` absent.** Not composed here — buffering is the leaf's concern
  (see Non-goals). The seven built layers wrap the leaf directly.

### Layer nesting (built layers only)

| Position | Layer | Config source | Invariant it anchors |
| --- | --- | --- | --- |
| outermost | `Tracing` | `timer` | one span over the whole logical request |
| | `CircuitBreaker` | `cfg.circuit_breaker`, `timer` | **outside `Retry`** — short-circuits before retry runs |
| | `Retry` | `cfg.retry`, `timer` | order-safe, retryability-aware |
| | `RateLimit` | `rate_limits`, `timer`, `cfg.rate_limit_max_wait` | **inside `Retry`** — each attempt spends budget |
| | `Timeout` | `cfg.timeout`, `timer` | bounds the send, **not** the permit wait |
| | `SetHeaders` | `cfg.headers` | static stamp, just outside `Auth` |
| innermost layer | `Auth` | `auth` | re-stamps credentials **per attempt** |
| leaf | `S` (inline double in tests / hyper in prod) | — | — |

## Testing

Full-stack over `stack(<inline leaf>, …, MockTimer)` — the only tests that catch a
builder reorder (per-layer isolation tests cannot).

**Leaf: an inline recording double, NOT `MockClient`.** `net-http-api` must not
dev-depend on `oath-adapter-net-http-mock` — that crate normal-depends on
`net-http-api`, so the dev-dep closes a cycle that recompiles a second, non-unifying
copy of the crate (the same constraint every layer suite already follows; see
`net-http-api-test-doubles` note). And `MockClient` is a fixed-status leaf — it cannot
script a `503 → 200` sequence or a never-completing send. So the stack tests build a
small **inline** leaf in the test module: a `Clone` struct with `Arc<Mutex<Vec<..>>>`
recording + a scripted/hanging response, satisfying `HttpClient + Clone + Send + Sync +
'static`, exactly like `retry.rs`/`circuit_breaker.rs`'s `ScriptLeaf`. `MockTimer`
comes from `oath-adapter-net-mock` (an existing dev-dep); tests run on `tokio`
(dev-only). No new dev-dependency.

1. **`CircuitBreaker` outside `Retry`** — with the circuit forced open (a leaf scripted
   to fail past `failure_threshold`, then a follow-up request), the leaf is **not**
   called again and `Retry` does not spin: assert the send count does not increase and
   the outcome is `HttpError::CircuitOpen`.
2. **`RateLimit` inside `Retry`** — a leaf scripted `503 → 200` under a rate bucket:
   assert the second attempt spends a token (advancing `MockTimer` is required between
   attempts for the token to refill), proving the limiter sits inside the retry loop.
3. **`Timeout` bounds the send, not the permit wait** — a leaf whose send never
   completes; advancing `MockTimer` past `cfg.timeout` yields `HttpError::Timeout`.
4. **`Auth` re-stamps per attempt** — a recording inline leaf + a counter `AuthSource`
   whose `authorize` stamps a monotonic header: assert each of the N attempts carries a
   distinct credential.
5. **Boot coverage** — `stack()` with a `RateLimitConfig` missing a `K::all()` variant
   returns `Err(BuildError::UndeclaredKey)` and constructs nothing.
6. **`Scope` fail-closed end-to-end** — a request with **no** `RateScope<K>` extension,
   driven through the fully assembled stack, is rejected (`HttpError::Throttled`,
   non-retryable — `Retry` does not spin), confirming the fail-closed path survives
   composition.
7. **Ordering-sanity smoke** — a plain `200` request threads through all seven layers
   and returns the leaf's body intact (transparency end-to-end).

## Delivery

One PR (`feat(net): stack() assembly + HttpConfig (Slice 2)`), one issue, one worktree
under `.claude/worktrees/<slug>`. No new dependency (so `deny`/`machete` are
unaffected). Records an ADR-0034 append-only amendment and a `CHANGELOG.md`
`[Unreleased]` entry. DoD: `just ci` green (incl. `just doc`).

## ADR reconciliation

Append an ADR-0034 amendment recording: `HttpConfig`'s five-field shape (including
`rate_limit_max_wait`, which feeds `RateLimitLayer::new` and is distinct from the send
`timeout` because `RateLimit` sits outside `Timeout`) and its non-generic/`serde`-free
rationale; `stack()`'s construct-RateLimit-first validation (delegating to
`RateLimitLayer::new`'s internal `validate_coverage` + `validate_concurrency_singleton`,
not a separate call) and compose contract; the exact nesting of the seven built layers
with `Auth`/`SetHeaders` as direct wrappers; the `BufferOrStream`-is-leaf-side
resolution; and that `build()` (next slice) delegates to this `stack()` over the hyper
leaf.
