# net-http `stack()` assembly + `HttpConfig` (Slice 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the runtime-free assembly for the net-http resilience stack — the non-generic `HttpConfig` aggregate and the `stack<S, T, A, K>()` function that validates pacing coverage and composes the ADR-0031 canonical layer order over any leaf — plus the full-stack ordering-invariant tests that only an assembly makes possible.

**Architecture:** `stack()` builds the one fallible layer (`RateLimit`, which validates coverage + concurrency-singleton) first, pre-wraps the leaf with the two direct `Service` wrappers (`Auth`, `SetHeaders`), then composes the five `Layer`-factory layers over that via the kernel's `ServiceBuilder` (first `.layer()` = outermost). The composed value auto-satisfies `HttpClient` via blanket impl; the return bound `impl HttpClient + Clone + Send + Sync + 'static` turns any layer regression into a compile error *at `stack()`*. No new dependency; no existing layer changes.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, the kernel's `ServiceBuilder`/`Layer`/`Timer` (`oath-adapter-net-api`), the already-shipped layers/config of `oath-adapter-net-http-api`. Tests use inline service doubles + `MockTimer` (`oath-adapter-net-mock`), driven on `tokio` (dev-only).

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic and no truncating `as` casts in non-test code** — return `Result`. Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations`), `const fn` where `missing_const_for_fn` asks.
- **`just doc` per task** — `just check`/`lint`/`test` do **not** catch broken rustdoc intra-doc links; every task's verify step runs `just doc`.
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde` in non-dev deps. **This slice adds no dependency at all** (prod or dev), so `deny`/`machete` are unaffected.
- **net-http-api tests must NOT dev-depend on `oath-adapter-net-http-mock` (`MockClient`)** — it normal-depends on this crate, so a dev-dep closes a cycle that recompiles a second, non-unifying copy. Use **inline** service doubles + `oath-adapter-net-mock`'s `MockTimer`, exactly as `retry.rs`/`circuit_breaker.rs`/`rate_limit.rs` do.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-05-net-http-stack-assembly-design.md](../specs/2026-07-05-net-http-stack-assembly-design.md), governed by [ADR-0031 §1](../../adr/0031-http-resilience-venue-pacing.md) (canonical layer order), the [construction-surface spec](../specs/2026-06-30-net-http-construction-surface-design.md) Seam #3, and [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 2 (assembly)**, runtime-free half; the hyper leaf + `build()` + `TokioTimer` are the following slice. Everything `stack()` composes already ships: all five layers (#76/#78/#82/#85/#86), `Auth`/`SetHeaders`/`Guarded` (#66), `RateScope`/`Scope` (#76), and `RateKey`/`RateLimitConfig`/`validate_coverage` (#72).

## File Structure

- `crates/adapter/net/http/api/src/stack.rs` — **new** (Tasks 1–2). `HttpConfig`, `stack()`, and the full test module (inline leaf + `CounterAuth` + test `RateKey` + all seven tests).
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Task 1). `pub mod stack;` + re-exports + module-doc bullet.
- `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`, `CHANGELOG.md` — **modify** (Task 3).

No `Cargo.toml` change — no new dependency. Each task is one or more commits; the tasks together are one PR/issue.

---

## Setup: issue (worktree already exists)

> The isolated worktree **already exists** at `.claude/worktrees/net-http-stack-assembly` (branch `feat/net-http-stack-assembly`, branched off `main`), and already holds the design spec + this plan (commit `docs(net): Slice 2 …`). All tasks run inside it. Only the GitHub issue remains.

- [ ] **Create the issue**

```bash
gh issue create \
  --title "feat(net): stack() assembly + HttpConfig (Slice 2)" \
  --label enhancement \
  --body "Slice 2 (assembly, runtime-free) of the net-http construction surface (spec: docs/superpowers/specs/2026-07-05-net-http-stack-assembly-design.md; ADR-0031 §1, ADR-0034).

- \`HttpConfig\` — non-generic aggregate (timeout, retry, circuit_breaker, headers, rate_limit_max_wait).
- \`stack<S, T, A, K>()\` — validates pacing coverage (via \`RateLimitLayer::new\`), then composes the canonical order \`Tracing(CircuitBreaker(Retry(RateLimit(Timeout(SetHeaders(Auth(leaf)))))))\` over any leaf; returns \`Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>\`.
- Full-stack ordering-invariant / boot-coverage / fail-closed tests over an inline recording leaf + \`MockTimer\` (no \`MockClient\` — dev-dep cycle).

No new dependency; no runtime; no existing-layer changes. The hyper leaf + \`build()\` are the following slice."
```

Note the issue number `#<N>` for the PR body.

---

## Task 1: `HttpConfig` + `stack()` — compose, validate, and prove it type-checks

**Files:**
- Create: `crates/adapter/net/http/api/src/stack.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes (all crate-local, already shipped): `HttpClient` (`client`); `Auth`, `SetHeaders`, `AuthSource`, `NoAuth` (`auth`); `TracingLayer` (`trace`); `CircuitBreakerLayer`, `CircuitBreakerConfig` (`circuit_breaker`); `RetryLayer`, `RetryConfig` (`retry`); `RateLimitLayer` (`rate_limit`); `TimeoutLayer` (`timeout`); `RateLimitConfig`, `RateKey`, `BuildError` (`rate`); and `ServiceBuilder`, `Layer`, `Timer` (`oath_adapter_net_api`).
- Produces:
  - `oath_adapter_net_http_api::HttpConfig` — `#[derive(Debug, Clone)] struct { pub timeout: Duration, pub retry: RetryConfig, pub circuit_breaker: CircuitBreakerConfig, pub headers: http::HeaderMap, pub rate_limit_max_wait: Duration }`.
  - `oath_adapter_net_http_api::stack` — `pub fn stack<S, T, A, K>(leaf: S, cfg: HttpConfig, timer: T, auth: A, rate_limits: RateLimitConfig<K>) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError> where S: HttpClient + Clone + Send + Sync + 'static, T: Timer + 'static, A: AuthSource + 'static, K: RateKey + std::fmt::Debug`.

> **Bound note (deviation from the spec sketch).** The spec sketch wrote `T: Timer, A: AuthSource, K: RateKey`. The real bounds add `+ 'static` on `T`/`A` (the composed value is returned as `'static`, and `Timer`/`AuthSource` are not `'static` by default) and `+ std::fmt::Debug` on `K` (`RateLimitLayer::new` renders the offending key when coverage fails). These are necessary, mechanical additions — record them in the ADR amendment (Task 3).

- [ ] **Step 1: Write the failing tests (module scaffolding + smoke + boot-coverage)**

Create `crates/adapter/net/http/api/src/stack.rs` with the module doc and **only** the test module below (the `HttpConfig`/`stack` items land in Step 4, so this compiles to a failure until then):

```rust
//! The `stack()` assembly (ADR-0031 §1) + the non-generic `HttpConfig`.
//!
//! [`stack`] composes the canonical resilience order over any leaf:
//! `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders( Auth( leaf ) ) ) ) ) ) )`.
//! It builds the one fallible layer ([`RateLimitLayer`](crate::RateLimitLayer),
//! which validates pacing coverage + the concurrency-singleton invariant) first,
//! so a coverage/param error is a [`BuildError`](crate::BuildError) before the
//! rest is assembled. `Auth`/`SetHeaders` are direct `Service` wrappers (no
//! `Layer` factory), so they pre-wrap the leaf; the five `Layer`-factory layers
//! compose over that via [`ServiceBuilder`](oath_adapter_net_api::ServiceBuilder)
//! (first `.layer()` = outermost). The composed value satisfies
//! [`HttpClient`](crate::HttpClient) by blanket impl.

#[cfg(test)]
mod tests {
    use super::{stack, HttpConfig};
    use crate::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use crate::{
        AuthSource, BuildError, CircuitBreakerConfig, HttpError, NoAuth, RateKey, RateScope,
        Retryable, RetryConfig, Scope, Service,
    };
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_mock::MockTimer;
    use std::collections::HashMap;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    // ---- test RateKey ----------------------------------------------------
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Snapshot,
        History,
    }
    impl RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Snapshot, Self::History]
        }
    }

    // ---- canned one-frame body (Data = Bytes, Error = HttpError) ----------
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(b: &'static [u8]) -> Self {
            Self { data: Some(Bytes::from_static(b)) }
        }
    }
    impl Body for StubBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.data.is_none()
        }
        fn size_hint(&self) -> SizeHint {
            self.data
                .as_ref()
                .map_or_else(|| SizeHint::with_exact(0), |d| SizeHint::with_exact(d.len() as u64))
        }
    }

    // ---- scripted, recording, clock-aware inline leaf --------------------
    // One scripted outcome per attempt; repeats the last once exhausted. Records
    // the `Authorization` header each call saw (for the Auth re-stamp test) and
    // counts calls (for the untouched-leaf assertions). Inline, not `MockClient`,
    // to avoid the net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone, Copy)]
    enum Step {
        Status(u16),
        Err,  // connection error (retryable)
        Hang, // sleeps 1h on the shared clock (for the Timeout test)
    }
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
        seen_auth: Arc<Mutex<Vec<Option<String>>>>,
        timer: MockTimer,
    }
    impl ScriptLeaf {
        fn new(timer: MockTimer, steps: Vec<Step>) -> Self {
            Self {
                steps: Arc::new(steps),
                calls: Arc::new(AtomicUsize::new(0)),
                seen_auth: Arc::new(Mutex::new(Vec::new())),
                timer,
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
        fn seen_auth(&self) -> Vec<Option<String>> {
            self.seen_auth.lock().unwrap().clone()
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self.steps.get(i).copied().unwrap_or_else(|| *self.steps.last().unwrap());
            let seen = req
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            self.seen_auth.lock().unwrap().push(seen);
            let timer = self.timer.clone();
            async move {
                match step {
                    Step::Status(code) => {
                        let mut resp = http::Response::new(StubBody::new(b"ok"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    }
                    Step::Err => Err(HttpError::connection("reset")),
                    Step::Hang => {
                        timer.sleep(Duration::from_secs(3600)).await;
                        Ok(http::Response::new(StubBody::new(b"late")))
                    }
                }
            }
        }
    }

    // ---- an AuthSource stamping a monotonically-increasing credential -----
    #[derive(Clone)]
    struct CounterAuth {
        n: Arc<AtomicUsize>,
    }
    impl CounterAuth {
        fn new() -> Self {
            Self { n: Arc::new(AtomicUsize::new(0)) }
        }
    }
    impl AuthSource for CounterAuth {
        fn authorize(
            &self,
            req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let n = self.n.fetch_add(1, Ordering::Relaxed);
            let val = http::HeaderValue::from_str(&format!("token-{n}")).unwrap();
            req.headers_mut().insert(http::header::AUTHORIZATION, val);
            std::future::ready(Ok(()))
        }
    }

    // ---- config builders --------------------------------------------------
    // Retry/circuit-breaker knobs tuned so pacing never accidentally interferes:
    // a generous global bucket, zero backoff (retries run inline under MockTimer).
    fn http_cfg(retry_attempts: u32, timeout: Duration, max_wait: Duration) -> HttpConfig {
        HttpConfig {
            timeout,
            retry: RetryConfig {
                max_attempts: NonZeroU32::new(retry_attempts).unwrap(),
                base: Duration::ZERO,
                cap: Duration::ZERO,
                seed: 1,
            },
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: NonZeroU32::new(3).unwrap(),
                cooldown: Duration::from_secs(30),
                throttle_cooldown: Duration::from_secs(900),
                half_open_probes: NonZeroU32::new(1).unwrap(),
            },
            headers: http::HeaderMap::new(),
            rate_limit_max_wait: max_wait,
        }
    }
    // Global effectively unlimited; Snapshot 2/s; History concurrency 1.
    fn rate_cfg() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 1000, per: Duration::from_secs(1), burst: 1000 },
            local: HashMap::from([
                (
                    Key::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 2,
                        per: Duration::from_secs(1),
                        burst: 2,
                    }),
                ),
                (Key::History, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
            ]),
        }
    }
    fn req(scope: Scope, key: Option<Key>) -> http::Request<Bytes> {
        let mut r = http::Request::builder().method("GET").uri("/x").body(Bytes::new()).unwrap();
        r.extensions_mut().insert(RateScope { scope, key });
        r.extensions_mut().insert(Retryable); // opt in so Retry engages when max_attempts > 1
        r
    }

    // ---- Task 1 tests -----------------------------------------------------

    #[tokio::test]
    async fn plain_request_threads_all_layers_and_body_is_transparent() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(leaf, http_cfg(1, Duration::from_secs(30), Duration::ZERO), timer, NoAuth, rate_cfg())
            .expect("total config");
        let resp = svc.call(req(Scope::Global, None)).await.expect("200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // through all 7 layers + Guarded, untouched
    }

    #[test]
    fn missing_key_is_a_build_error_and_constructs_nothing() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let mut rc = rate_cfg();
        rc.local.remove(&Key::History); // no longer total over Key::all()
        let err = stack(leaf, http_cfg(1, Duration::from_secs(30), Duration::ZERO), timer, NoAuth, rc)
            .unwrap_err();
        assert!(matches!(err, BuildError::UndeclaredKey(ref k) if k.contains("History")));
    }
}
```

Then wire `lib.rs`. Add the module-doc bullet after the `trace` bullet (the `//! - [\`trace\`] …` block):

```rust
//! - [`stack`] — `HttpConfig` and the `stack()` assembly composing the canonical
//!   resilience order (ADR-0031 §1) over any leaf (Slice 2)
```

Add the module declaration alphabetically, between `pub mod service;` and `pub mod timeout;`:

```rust
pub mod stack;
```

Add the re-export alphabetically, between `pub use service::Service;` and `pub use timeout::{…};`:

```rust
pub use stack::{stack, HttpConfig};
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find function stack` / `cannot find type HttpConfig` in module `stack` (only the test module exists).

- [ ] **Step 3: (skipped — no separate red step)**

The failing compile in Step 2 is the red. Proceed to implement.

- [ ] **Step 4: Implement `HttpConfig` + `stack()`**

Insert between the module doc and the `#[cfg(test)] mod tests` in `stack.rs`:

```rust
use crate::rate::{BuildError, RateKey, RateLimitConfig};
use crate::{
    Auth, AuthSource, CircuitBreakerConfig, CircuitBreakerLayer, HttpClient, RateLimitLayer,
    RetryConfig, RetryLayer, SetHeaders, TimeoutLayer, TracingLayer,
};
use oath_adapter_net_api::{Layer, ServiceBuilder, Timer};
use std::fmt;
use std::time::Duration;

/// Non-generic assembly configuration: one field per configurable layer plus the
/// static default headers. The `K`-generic pacing map (`RateLimitConfig<K>`),
/// `auth`, and `timer` are separate [`stack`] arguments, so this type carries no
/// type parameter and no `serde` (deserialisation stays in the adapter, ADR-0003).
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Per-attempt send timeout — bounds the send, **not** the permit wait.
    pub timeout: Duration,
    /// Retry policy (attempts, backoff schedule).
    pub retry: RetryConfig,
    /// Circuit-breaker thresholds and cooldowns.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Static default request headers, stamped by `SetHeaders` (just outside `Auth`).
    pub headers: http::HeaderMap,
    /// Ceiling on how long an exhausted bucket back-pressures before the request
    /// returns [`HttpError::Throttled`](crate::HttpError::Throttled). Distinct from
    /// `timeout`: `RateLimit` sits **outside** `Timeout`, so the permit wait is
    /// bounded by this — at IBKR's 1/15-min buckets, minutes not seconds.
    pub rate_limit_max_wait: Duration,
}

/// Assemble the canonical resilience stack (ADR-0031 §1) over an arbitrary leaf.
///
/// Builds the fallible [`RateLimit`](crate::RateLimit) layer **first** — it runs
/// `validate_coverage` + `validate_concurrency_singleton` — so a config that is not
/// total over `K::all()`, carries an out-of-range policy param, or breaches the
/// ≤1-concurrency-permit invariant is a [`BuildError`] before the infallible layers
/// are assembled. Then composes, outermost-first:
/// `Tracing( CircuitBreaker( Retry( RateLimit( Timeout( SetHeaders( Auth( leaf ) ) ) ) ) ) )`.
/// `Auth`/`SetHeaders` are direct `Service` wrappers (no `Layer` factory), so they
/// pre-wrap the leaf; the composed value satisfies [`HttpClient`] by blanket impl.
///
/// # Errors
/// [`BuildError`] propagated from `RateLimitLayer::new` if `rate_limits` is not
/// total over `K::all()`, any policy is out of range, or the concurrency-singleton
/// invariant is breached.
pub fn stack<S, T, A, K>(
    leaf: S,
    cfg: HttpConfig,
    timer: T,
    auth: A,
    rate_limits: RateLimitConfig<K>,
) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>
where
    S: HttpClient + Clone + Send + Sync + 'static,
    T: Timer + 'static,
    A: AuthSource + 'static,
    K: RateKey + fmt::Debug,
{
    // Fallible layer first: validates coverage + concurrency-singleton (fail-closed
    // at construction — nothing else is built if this errors).
    let rate = RateLimitLayer::new(&rate_limits, timer.clone(), cfg.rate_limit_max_wait)?;
    // The two innermost layers are direct wrappers, not `Layer` factories.
    let inner = SetHeaders::new(Auth::new(leaf, auth), cfg.headers);
    let svc = ServiceBuilder::new()
        .layer(TracingLayer::new(timer.clone())) // outermost
        .layer(CircuitBreakerLayer::new(cfg.circuit_breaker, timer.clone()))
        .layer(RetryLayer::new(cfg.retry, timer.clone()))
        .layer(rate)
        .layer(TimeoutLayer::new(cfg.timeout, timer)) // innermost Layer-factory
        .service(inner);
    Ok(svc)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api stack && just lint && just doc`
Expected: PASS, warning-free, docs clean. The smoke test proves the composed type type-checks against the `impl HttpClient + Clone + Send + Sync + 'static` bound and is body-transparent; the boot-coverage test proves construction fails closed.

> Known risks (address in place if hit):
> - **Return-bound compile error.** If `stack()` fails to satisfy `Send`/`Sync`/`'static`, the failing layer is named in the error. This is the bound doing its job — the fix is on the layer, not `stack()`; but for this slice every shipped layer already holds `Arc` state + `Clone` config, so a failure most likely means a missing `+ 'static` on `T`/`A` (already in the `where` clause above) or a non-`Sync` test leaf (the `ScriptLeaf` is `Sync` via its `Arc` fields).
> - **`ServiceBuilder` import.** `.layer()`/`.service()` are inherent methods; the `Layer` trait is imported only to satisfy the `where L: Layer<S>` bound resolution. If clippy flags `Layer` as unused, drop it from the `use`.
> - **`missing_const_for_fn` on `stack`.** It calls fallible `RateLimitLayer::new`, so it cannot be `const` — no action.
> - **`LimitPolicy::TokenBucket` fields.** It has `{ rate, per, burst }` (three fields). The `rate_cfg()` builder uses all three — do not drop `per`.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/http/api/src/stack.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): HttpConfig + stack() — validate-then-compose the canonical order"
```

---

## Task 2: Full-stack ordering-invariant, fail-closed, and Auth-per-attempt tests

**Files:**
- Modify: `crates/adapter/net/http/api/src/stack.rs` (test module only)

**Interfaces:**
- Consumes: the Task 1 test harness (`ScriptLeaf`, `CounterAuth`, `Key`, `StubBody`, `http_cfg`, `rate_cfg`, `req`) and `stack`/`HttpConfig` (Task 1).
- Produces: no new public items — five regression tests that lock the composition order. These pass immediately against a **correct** `stack()`; if any fails, `stack.rs`'s layer order is wrong and must be fixed there (that is the point of full-stack tests — per-layer tests cannot catch a reorder).

> These are characterization tests over already-built behaviour, so the TDD "red" is structural: run each once and confirm it passes (a correct order), and reason about *why the wrong order would fail* (noted per test). If one is red, the composition is genuinely wrong — fix `stack()`.

- [ ] **Step 1: Add the five tests**

Append inside the same `tests` module in `stack.rs`, after the Task 1 tests. **No new imports** — these tests use only names already in scope from Task 1 (`ScriptLeaf`, `Step`, `CounterAuth`, `Key`, `stack`, `http_cfg`, `rate_cfg`, `req`, `RateLimitConfig`, `LimitDecl`, `LimitPolicy`, `HttpError`, `NoAuth`, `Scope`, `MockTimer`, `Duration`, `HashMap`). Add the tests:

```rust
    // 1. CircuitBreaker OUTSIDE Retry — an open circuit fast-rejects; the leaf is
    //    untouched and no retry loop spins on the rejection. If CB were INSIDE
    //    Retry this could not hold: the breaker would be re-consulted per attempt.
    #[tokio::test]
    async fn circuit_opens_and_fast_rejects_without_touching_the_leaf() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Err]); // always fails
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO), // retry ON (3), zero backoff
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        // 3 logical failures (each retried 3x → 9 leaf calls) trip the breaker.
        for _ in 0..3 {
            let _ = svc.call(req(Scope::Global, None)).await;
        }
        let calls_after_trip = leaf.calls();
        assert_eq!(calls_after_trip, 9, "3 requests x 3 attempts reached the leaf before the trip");
        // Next request: circuit is Open → CircuitOpen, leaf untouched, no spin.
        let err = svc.call(req(Scope::Global, None)).await.unwrap_err();
        assert!(matches!(err, HttpError::CircuitOpen));
        assert_eq!(leaf.calls(), 9, "open circuit fast-rejects; leaf untouched, Retry never spun");
    }

    // 2. RateLimit INSIDE Retry — each attempt re-acquires budget. With a burst-1
    //    bucket and zero max_wait, the first attempt drains it and the retry
    //    throttles at the (empty) bucket, so the leaf is hit exactly once. If
    //    RateLimit were OUTSIDE Retry, the single token would cover the whole
    //    logical request and the retry would resend to a 200.
    #[tokio::test]
    async fn rate_limit_is_spent_per_attempt_inside_retry() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(503), Step::Status(200)]);
        // Snapshot: burst 1, refill 1/hour → no refill during the test.
        let rc = RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 1000, per: Duration::from_secs(1), burst: 1000 },
            local: HashMap::from([
                (
                    Key::Snapshot,
                    LimitDecl::Policy(LimitPolicy::TokenBucket {
                        rate: 1,
                        per: Duration::from_secs(3600),
                        burst: 1,
                    }),
                ),
                (Key::History, LimitDecl::GlobalOnly),
            ]),
        };
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rc,
        )
        .expect("total config");
        let err = svc.call(req(Scope::Local, Some(Key::Snapshot))).await.unwrap_err();
        assert!(
            matches!(err, HttpError::Throttled),
            "the retry re-acquired the drained bucket → per-attempt pacing (RateLimit inside Retry)"
        );
        assert_eq!(leaf.calls(), 1, "only attempt 1 reached the leaf; the retry throttled at the bucket");
    }

    // 3. Timeout bounds the SEND. A hanging leaf, with the clock advanced past the
    //    send timeout, yields Timeout. (RateLimit sits outside Timeout, so the
    //    permit wait is bounded separately by rate_limit_max_wait — structural.)
    #[tokio::test]
    async fn send_timeout_fires_on_a_hanging_leaf() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Hang]);
        let svc = stack(
            leaf,
            http_cfg(1, Duration::from_secs(1), Duration::ZERO), // retry OFF, 1s send timeout
            timer.clone(),
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        let waiter = tokio::spawn(async move { svc.call(req(Scope::Global, None)).await });
        tokio::task::yield_now().await; // task registers the inner sleep + the 1s deadline
        timer.advance(Duration::from_secs(1)); // fire the send-timeout deadline
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Timeout));
    }

    // 4. Auth re-stamps per attempt — inside Retry, so each of the N attempts
    //    carries a fresh credential.
    #[tokio::test]
    async fn auth_restamps_a_fresh_credential_on_every_attempt() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Err, Step::Err, Step::Status(200)]);
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            CounterAuth::new(),
            rate_cfg(),
        )
        .expect("total config");
        let resp = svc.call(req(Scope::Global, None)).await.expect("third attempt is 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(
            leaf.seen_auth(),
            vec![Some("token-0".to_owned()), Some("token-1".to_owned()), Some("token-2".to_owned())],
            "Auth ran once per attempt (inside Retry), re-stamping a fresh credential each time"
        );
    }

    // 5. Scope fail-closed end-to-end — a request with no RateScope extension is
    //    rejected before the leaf, and the fail-closed path survives composition.
    #[tokio::test]
    async fn absent_scope_is_rejected_fail_closed_through_the_stack() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(
            leaf.clone(),
            http_cfg(3, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        // A request with neither RateScope nor Retryable — the forgotten-stamp case.
        let bare = http::Request::builder().method("GET").uri("/x").body(Bytes::new()).unwrap();
        let err = svc.call(bare).await.unwrap_err();
        assert!(matches!(err, HttpError::Throttled), "no RateScope → fail-closed Throttled");
        assert_eq!(leaf.calls(), 0, "the fail-closed request never reached the leaf");
    }
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p oath-adapter-net-http-api stack -- --nocapture`
Expected: PASS — all seven `stack` tests green. A red test #1–#5 means the composition order in `stack()` is wrong (or a layer's classification changed); fix `stack.rs` (do not weaken the test).

- [ ] **Step 3: Lint + doc**

Run: `just lint && just doc`
Expected: PASS, warning-free, docs clean. (The `_AtomicUsizeInUse` scaffolding line from Step 1, if you added it, is unnecessary — remove it; all names are already imported by Task 1.)

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/stack.rs
git commit -m "test(net): full-stack ordering-invariant, fail-closed, and per-attempt Auth tests"
```

---

## Task 3: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Append to ADR-0034's **Amendments** numbered list, after item **10** (the Tracing note), a new item **11**:

```markdown
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
    RateKey + Debug` in the implementation (the composed value is returned `'static`;
    coverage validation renders the offending key). `BufferOrStream` is **not** a
    layer here — buffering is a leaf-side body-construction concern, so the innermost
    leaf already satisfies "inside `Retry`". Full-stack ordering invariants are
    regression-tested over an inline recording leaf + `MockTimer` (not `MockClient`,
    which would close the net-http-mock → net-http-api dev-dep cycle and cannot script
    sequences). No new dependency; no existing-layer change.
```

> **Numbering caveat:** if another PR landed an amendment concurrently and took **#11**, renumber this one to the next free integer during rebase (mechanical, not a design change).

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added`, at the **end of the list** (after the `Tracing` resilience-layer entry):

```markdown
- `oath-adapter-net-http-api` `stack()` assembly + `HttpConfig` (Slice 2, assembly) —
  `stack<S, T, A, K>()` composes the canonical resilience order (ADR-0031 §1)
  `Tracing(CircuitBreaker(Retry(RateLimit(Timeout(SetHeaders(Auth(leaf)))))))` over any
  leaf, returning `Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>`.
  It builds the fallible `RateLimit` layer first (running `validate_coverage` +
  `validate_concurrency_singleton`), so a coverage/param/singleton failure is a boot
  error before the rest is assembled. `HttpConfig` is the non-generic aggregate
  (`timeout`, `retry`, `circuit_breaker`, `headers`, `rate_limit_max_wait`); the pacing
  map, `auth`, and `timer` are separate arguments. Full-stack ordering invariants
  (CircuitBreaker-outside-Retry, RateLimit-inside-Retry, send-Timeout, per-attempt
  Auth, Scope fail-closed) are regression-tested over an inline leaf + `MockTimer`. No
  new dependency; no existing-layer change. (ADR-0031 §1, ADR-0034.) The hyper leaf +
  `build()` land in the following slice.
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green — fmt, lint, test + doctests, doc, deny, typos, machete. No new dependency, so `deny`/`machete` see no change.

- [ ] **Step 4: Commit, push, PR**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md
git commit -m "docs(net): record stack()/HttpConfig assembly amendment (ADR-0034) + changelog"
git push -u origin feat/net-http-stack-assembly
gh pr create \
  --title "feat(net): stack() assembly + HttpConfig (Slice 2)" \
  --body "Closes #<N>

Slice 2 (assembly, runtime-free) of the net-http construction surface (spec: docs/superpowers/specs/2026-07-05-net-http-stack-assembly-design.md; ADR-0031 §1, ADR-0034).

- **\`HttpConfig\`** — non-generic aggregate: \`timeout\`, \`retry\`, \`circuit_breaker\`, \`headers\`, \`rate_limit_max_wait\`.
- **\`stack<S, T, A, K>()\`** — builds the fallible \`RateLimit\` layer first (validates coverage + concurrency-singleton), then composes \`Tracing(CircuitBreaker(Retry(RateLimit(Timeout(SetHeaders(Auth(leaf)))))))\` over any leaf. Returns \`Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>\`, so a layer regression is a compile error at \`stack()\`.
- **Full-stack tests** over an inline recording leaf + \`MockTimer\` (no \`MockClient\` — dev-dep cycle): CircuitBreaker-outside-Retry, RateLimit-inside-Retry, send-Timeout, per-attempt Auth re-stamp, Scope fail-closed, plus a transparency smoke and a boot-coverage \`BuildError\`.

No new dependency; no runtime; no existing-layer change. \`BufferOrStream\` is leaf-side (not a layer). Next: the hyper-backend slice — \`build()\` + \`hyper_leaf()\` + \`TokioTimer\`, delegating to this \`stack()\`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `HttpConfig` five-field non-generic aggregate (`timeout`, `retry`, `circuit_breaker`, `headers`, `rate_limit_max_wait`) — Task 1 Step 4. ✅
- `stack<S, T, A, K>()`, validate-via-RateLimit-first, canonical order, full return bound — Task 1 Step 4 + smoke test. ✅
- `Auth`/`SetHeaders` as direct wrappers pre-wrapping the leaf; five `Layer` layers via `ServiceBuilder` — Task 1 Step 4. ✅
- Boot coverage → `BuildError::UndeclaredKey`, constructs nothing — Task 1 `missing_key_is_a_build_error_and_constructs_nothing`. ✅
- CircuitBreaker outside Retry — Task 2 test 1. ✅
- RateLimit inside Retry (per-attempt budget) — Task 2 test 2. ✅
- Timeout bounds the send — Task 2 test 3. ✅
- Auth re-stamps per attempt — Task 2 test 4. ✅
- Scope fail-closed end-to-end — Task 2 test 5. ✅
- Transparency smoke (all seven layers) — Task 1 `plain_request_threads_all_layers_and_body_is_transparent`. ✅
- Inline leaf + `MockTimer`, no `MockClient` — Tasks 1–2 harness. ✅
- ADR-0034 amendment #11 + CHANGELOG — Task 3. ✅
- Deferred (correctly absent): `build()`, `hyper_leaf`, `ConnConfig`, `TokioTimer`, `BufferOrStream` layer, `serde` on `HttpConfig` — noted, not built. ✅

**Placeholder scan:** none — every code step carries complete code; every command step an expected result. `#<N>` is the real issue number captured in Setup. (The `_AtomicUsizeInUse` line in Task 2 Step 1 is an explicit no-op guard removed in Step 3, not a placeholder.)

**Type consistency:**
- `HttpConfig { timeout, retry, circuit_breaker, headers, rate_limit_max_wait }` — identical in the struct (Task 1 Step 4), `http_cfg()` (Task 1 Step 1), and the ADR/CHANGELOG copy.
- `stack(leaf, cfg, timer, auth, rate_limits) -> Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>` with `S: HttpClient + Clone + Send + Sync + 'static, T: Timer + 'static, A: AuthSource + 'static, K: RateKey + fmt::Debug` — identical in the Interfaces block, Task 1 Step 4, and every test call site.
- `RateLimitLayer::new(&cfg, timer, max_wait) -> Result<_, BuildError>`, `Auth::new(inner, auth)`, `SetHeaders::new(inner, headers)`, `TracingLayer::new(timer)`, `CircuitBreakerLayer::new(cfg, timer)`, `RetryLayer::new(cfg, timer)`, `TimeoutLayer::new(default, timer)` — all match the shipped signatures read from the crate.
- `LimitPolicy::TokenBucket { rate, per, burst }` / `Concurrency { max }`, `LimitDecl::{Policy, GlobalOnly}`, `RateLimitConfig { global, local }` — match the shipped `rate.rs`/`rate_limit.rs`.
- `RetryConfig { max_attempts: NonZeroU32, base, cap, seed }`, `CircuitBreakerConfig { failure_threshold, cooldown, throttle_cooldown, half_open_probes }` (all `NonZeroU32`/`Duration`) — match the shipped structs.
- `Scope::{None, Global, Local, Both}`, `RateScope { scope, key: Option<K> }`, `Retryable`, `NoAuth`, `HttpError::{Throttled, Timeout, CircuitOpen, connection()}`, `BuildError::UndeclaredKey` — match the shipped enums/markers.
- `AuthSource::authorize(&self, &mut http::Request<Bytes>) -> impl Future<Output = Result<(), HttpError>> + Send` — `CounterAuth` matches the shipped trait exactly.
- lib.rs additions: `pub mod stack;` + `pub use stack::{stack, HttpConfig};` (module in the type namespace, fn in the value namespace — no clash) + one module-doc bullet.

**Known risks to watch during impl:** listed inline in Task 1 Step 5 (return-bound compile error, `ServiceBuilder`/`Layer` import, `const fn`, `TokenBucket` fields) and Task 2 Step 1/3 (the no-op import guard).
