# net-http `RateLimit` Layer (Slice 1, PR 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `RateLimit<S, K, T>` HTTP middleware layer that proactively paces every request against its global and per-endpoint token-bucket / concurrency limits — so the stack never hits IBKR's 429 penalty box — and fails **closed** on any coverage gap that reaches runtime.

**Architecture:** A `Timer`-generic, runtime-neutral `Service` wrapper in `oath-adapter-net-http-api`, built from the `RateLimitConfig<K>` that Slice 0 PR 4 landed. It reads a per-request `RateScope<K>` extension, acquires rate tokens (refilled from a `std::sync::Mutex`-guarded bucket, lock released before every `await`) then concurrency permits (an `async_lock::Semaphore` raced against a `max_wait` deadline via `futures_util::future::select`), and always returns `http::Response<Guarded<B>>` so a held concurrency permit rides the response body to stream-end.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `async-lock` (semaphore), `futures-util` (the acquire race — already a workspace dep), `http`/`http-body`/`bytes`, `std::sync::Mutex` + `std::time::{Duration, Instant}`. Tests use `MockClient` (`oath-adapter-net-http-mock`) + `MockTimer` (`oath-adapter-net-mock`), driven on `tokio` (dev-only) and via hand-polling on `Waker::noop()` for runtime-neutrality.

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result`. Recover a poisoned `std::sync::Mutex` with `.unwrap_or_else(std::sync::PoisonError::into_inner)`, never `.lock().unwrap()`. Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations` — hand-impl where a derive would demand `Debug` on `S`/inner state), `const fn` where `missing_const_for_fn` asks, compare unsigned to `== 0` not `< 1`.
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde` in non-dev deps. This PR adds **one crate-level dep**, `futures-util` (already vetted in `[workspace.dependencies]`), so `cargo-deny`/`machete` see no new *workspace* dep.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree under `.claude/worktrees/<slug>` → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-04-net-http-ratelimit-layer-design.md](../specs/2026-07-04-net-http-ratelimit-layer-design.md), governed by [ADR-0031 §3–§4](../../adr/0031-http-resilience-venue-pacing.md) and [ADR-0034 §3](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 1, PR 1** — the first of five resilience-layer PRs (RateLimit, then Timeout, Retry, CircuitBreaker, Tracing).

## File Structure

- `crates/adapter/net/http/api/src/rate.rs` — **modify** (Task 1). Add `per: Duration` to `LimitPolicy::TokenBucket`; update `validate`; add `BuildError::MultipleConcurrency` + `validate_concurrency_singleton`; update existing tests.
- `crates/adapter/net/http/api/src/rate_limit.rs` — **new** (Tasks 2–4). `Scope`, `RateScope<K>`, `RateState<K>`, `Bucket`, `TokenState`, `RateLimit<S, K, T>`, `RateLimitLayer<K, T>`, and their tests.
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Tasks 2, 4). `pub mod rate_limit;` + re-exports + module-doc bullets; extend the `rate` re-export with `validate_concurrency_singleton`.
- `crates/adapter/net/http/api/Cargo.toml` — **modify** (Task 3). Add `futures-util`.
- `CHANGELOG.md`, `docs/adr/0034-...md` — **modify** (Task 5).

Each task is one or more commits; the tasks together are one PR/issue.

---

## Setup: issue + worktree

- [ ] **Create the issue and the isolated worktree**

```bash
gh issue create \
  --title "feat(net): RateLimit resilience layer (Slice 1, PR 1)" \
  --label enhancement \
  --body "Slice 1 PR 1 of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-ratelimit-layer-design.md; ADR-0031 §3-4).

- \`RateLimit<S, K, T>\` + \`RateLimitLayer<K, T>\` (impl \`net-api::Layer\`): proactive pacing so the stack never hits IBKR's 429 penalty box
- \`RateScope<K>\`/\`Scope\` per-request directive; absent -> fails closed; None -> opt-out; runtime coverage gap -> fail-closed \`Throttled\`
- \`LimitPolicy::TokenBucket\` gains \`per: Duration\` so IBKR sub-1/s limits are expressible; \`BuildError::MultipleConcurrency\` boot check (<=1 concurrency permit per request)"
```

Note the issue number `#<N>` for the PR body.

```bash
git worktree add .claude/worktrees/net-http-ratelimit -b feat/net-http-ratelimit main
cd .claude/worktrees/net-http-ratelimit
```

All subsequent tasks run inside this worktree.

---

## Task 1: `LimitPolicy` `per` amendment + `MultipleConcurrency` boot check

**Files:**
- Modify: `crates/adapter/net/http/api/src/rate.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: existing `RateKey`, `LimitDecl`, `RateLimitConfig`, `BuildError`, `validate_coverage`.
- Produces:
  - `LimitPolicy::TokenBucket { rate: u32, per: std::time::Duration, burst: u32 }` (was `{ rate, burst }`).
  - `BuildError::MultipleConcurrency` — a new fieldless variant.
  - `pub fn validate_concurrency_singleton<K: RateKey>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>`.

- [ ] **Step 1: Write the failing tests**

Add `use std::time::Duration;` to the top of `rate.rs`. In the `tests` module, extend the `use super::…` line to include `validate_concurrency_singleton`, and add these tests (they will not compile until Step 3):

```rust
    #[test]
    fn token_bucket_carries_a_period_for_sub_1_per_second_rates() {
        // IBKR orders = 1 per 5s — inexpressible as tokens/second under u32.
        let p = LimitPolicy::TokenBucket { rate: 1, per: Duration::from_secs(5), burst: 1 };
        assert!(matches!(p, LimitPolicy::TokenBucket { rate: 1, burst: 1, .. }));
    }

    #[test]
    fn zero_period_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, per: Duration::ZERO, burst: 5 }),
        );
        assert!(matches!(validate_coverage(&cfg), Err(BuildError::InvalidPolicy(_))));
    }

    #[test]
    fn global_and_local_concurrency_is_rejected() {
        // Both-scoped request would need two held permits; Guarded holds one.
        let cfg = RateLimitConfig {
            global: LimitPolicy::Concurrency { max: 5 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
                (TestKey::Snapshot, LimitDecl::GlobalOnly),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(validate_concurrency_singleton(&cfg), Err(BuildError::MultipleConcurrency));
    }

    #[test]
    fn global_rate_with_local_concurrency_is_allowed() {
        // The real IBKR shape: global 10/s rate + /history concurrency.
        assert_eq!(validate_concurrency_singleton(&total_config()), Ok(()));
    }
```

Update the existing `total_config()` helper and **every** `TokenBucket` literal already in the tests (in `config_classifies_every_key_explicitly`, `total_config`, `zero_rate_token_bucket_is_invalid`, `zero_burst_token_bucket_is_invalid`, `bad_global_policy_is_invalid`) to include `per`. The updated helper:

```rust
    fn total_config() -> RateLimitConfig<TestKey> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 10, per: Duration::from_secs(1), burst: 20 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
                (TestKey::Snapshot, LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, per: Duration::from_secs(1), burst: 5 })),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        }
    }
```

For the three negative token-bucket tests, add `per: Duration::from_secs(1)` to their `TokenBucket` literals (e.g. `TokenBucket { rate: 0, per: Duration::from_secs(1), burst: 5 }`).

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `missing field 'per' in initializer of LimitPolicy::TokenBucket`, `no variant MultipleConcurrency`, `cannot find function validate_concurrency_singleton`.

- [ ] **Step 3: Implement the amendment**

In `rate.rs`, add `use std::time::Duration;` at the top. Change the `TokenBucket` variant and its doc:

```rust
    /// A refilling token bucket: `rate` tokens per `per` window, up to `burst`
    /// in hand. `per` lets sub-1/second venue limits (IBKR `1/5s`, `1/min`,
    /// `1/15min`) be expressed exactly with integer parameters.
    TokenBucket {
        /// Tokens replenished per `per` window (must be `>= 1`).
        rate: u32,
        /// The replenishment window (must be non-zero).
        per: Duration,
        /// Maximum tokens available at once (must be `>= 1`).
        burst: u32,
    },
```

Extend `LimitPolicy::validate` — add a `per` check inside the `TokenBucket` arm, after the `burst` check:

```rust
                if per.is_zero() {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket period must be non-zero, got {per:?}"
                    )));
                }
```

(Update the arm pattern to `Self::TokenBucket { rate, per, burst }`.)

Add the `BuildError` variant after `InvalidPolicy`:

```rust
    /// A config in which a `Both`-scoped request could require two held
    /// concurrency permits (global `Concurrency` **and** a local `Concurrency`)
    /// — [`Guarded`](crate::Guarded) holds one, so this is a boot failure, not a
    /// silent runtime permit truncation.
    #[error(
        "config has both a global and a local Concurrency policy; a Both-scoped request would need two held permits (Guarded holds one)"
    )]
    MultipleConcurrency,
```

Add the validator after `validate_coverage`:

```rust
/// Reject a config whose `Both`-scoped requests could require two held
/// concurrency permits — global `Concurrency` **and** any local `Concurrency`.
/// `RateLimitLayer::new` calls this alongside [`validate_coverage`], turning the
/// ≤1-concurrency-permit invariant into a boot failure (spec Decision 6).
///
/// # Errors
/// [`BuildError::MultipleConcurrency`] if the global policy is `Concurrency` and
/// any local `Policy` is also `Concurrency`.
pub fn validate_concurrency_singleton<K>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>
where
    K: RateKey,
{
    if matches!(cfg.global, LimitPolicy::Concurrency { .. }) {
        for decl in cfg.local.values() {
            if matches!(decl, LimitDecl::Policy(LimitPolicy::Concurrency { .. })) {
                return Err(BuildError::MultipleConcurrency);
            }
        }
    }
    Ok(())
}
```

In `lib.rs`, extend the `rate` re-export:

```rust
pub use rate::{
    BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_concurrency_singleton,
    validate_coverage,
};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api rate && just lint`
Expected: PASS, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): LimitPolicy TokenBucket period + MultipleConcurrency boot check"
```

---

## Task 2: `Scope` + `RateScope<K>` — the per-request directive

**Files:**
- Create: `crates/adapter/net/http/api/src/rate_limit.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `RateKey`.
- Produces:
  - `oath_adapter_net_http_api::Scope` — `#[non_exhaustive] enum { None, Global, Local, Both }` (`Copy`).
  - `oath_adapter_net_http_api::RateScope<K>` — `struct { pub scope: Scope, pub key: Option<K> }` (`Clone`), an `http::Request` extension.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/rate_limit.rs` with the module doc + only the tests below, and wire it into `lib.rs` (Step 3):

```rust
//! The `RateLimit` resilience layer (ADR-0031 §3): proactive per-endpoint
//! pacing built from a validated [`RateLimitConfig`](crate::RateLimitConfig),
//! plus the per-request [`RateScope`] directive that selects which buckets a
//! request spends against. Runtime-neutral: generic over
//! [`Timer`](oath_adapter_net_api::Timer), semaphore via `async-lock`.

#[cfg(test)]
mod tests {
    use super::{RateScope, Scope};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum TestKey {
        History,
    }
    impl crate::RateKey for TestKey {
        fn all() -> &'static [Self] {
            &[Self::History]
        }
    }

    #[test]
    fn rate_scope_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut()
            .insert(RateScope { scope: Scope::Both, key: Some(TestKey::History) });
        let got = req
            .extensions()
            .get::<RateScope<TestKey>>()
            .cloned()
            .expect("directive present");
        assert!(matches!(got.scope, Scope::Both));
        assert_eq!(got.key, Some(TestKey::History));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type RateScope`/`Scope` in module `rate_limit`.

- [ ] **Step 3: Implement the directive types**

Insert between the module doc and the tests in `rate_limit.rs`:

```rust
/// Which bucket sets a request spends against (ADR-0031 §3). Stamped by the
/// adapter as part of a [`RateScope`] request extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    /// Acquire nothing — the **explicit** unlimited opt-out.
    None,
    /// Spend against the account-wide global bucket only.
    Global,
    /// Spend against this endpoint's local bucket only.
    Local,
    /// Spend against both the global and the local bucket.
    Both,
}

/// The per-request pacing directive, carried as an `http::Request` extension.
///
/// The adapter stamps it when it builds each request (it knows the endpoint).
/// An **absent** directive is **rejected fail-closed** (ADR-0034 Amendment #1) —
/// a forgotten stamp must not silently fly global-only. `Clone` so it survives the
/// per-attempt request clone `Retry` performs (Slice 1).
#[derive(Debug, Clone)]
pub struct RateScope<K> {
    /// Which bucket sets to spend against.
    pub scope: Scope,
    /// The endpoint key, required for `Local`/`Both`.
    pub key: Option<K>,
}
```

In `lib.rs`, add the module and re-export (keep alphabetical `pub mod`/`pub use` ordering — `rate_limit` sits after `rate`), and add a module-doc bullet:

```rust
//! - [`rate_limit`] — the `RateLimit` layer, its `RateLimitLayer` factory, and
//!   the `RateScope`/`Scope` per-request directive
```

```rust
pub mod rate_limit;
```

```rust
pub use rate_limit::{RateScope, Scope};
```

(Task 4 extends this to `pub use rate_limit::{RateLimit, RateLimitLayer, RateScope, Scope};`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api rate_limit && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): RateScope/Scope per-request pacing directive"
```

---

## Task 3: `RateLimit` layer — construction + acquire (rate tokens + concurrency permits)

**Files:**
- Modify: `crates/adapter/net/http/api/src/rate_limit.rs`
- Modify: `crates/adapter/net/http/api/Cargo.toml`

**Interfaces:**
- Consumes: `Scope`, `RateScope` (Task 2); `RateKey`, `RateLimitConfig`, `LimitPolicy`, `LimitDecl`, `BuildError`, `validate_coverage`, `validate_concurrency_singleton` (Task 1); `Guarded` (crate `body`); `HttpError`, `Service` (crate); `Timer` (`oath_adapter_net_api`); `async_lock::{Semaphore, SemaphoreGuardArc}`; `futures_util::future::{select, Either}`.
- Produces:
  - `oath_adapter_net_http_api::RateLimit<S, K, T>` — the wrapping `Service`.
  - `oath_adapter_net_http_api::RateLimitLayer<K, T>` — `impl Layer<S>` factory; `pub fn new(cfg: &RateLimitConfig<K>, timer: T, max_wait: Duration) -> Result<Self, BuildError>` where `K: RateKey + fmt::Debug, T: Timer`.
  - `RateLimit` returns `Response = http::Response<Guarded<B>>` for an inner `S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>`.

- [ ] **Step 1: Add the dependency**

In `crates/adapter/net/http/api/Cargo.toml`, add to `[dependencies]` (alphabetical, after `bytes`):

```toml
futures-util = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Replace the `use super::…` line in the `tests` module and add these tests (they need the `TokenBucket`/`Concurrency` grounding of the IBKR table). Add a `TokenBucket` and a `Concurrency` key to the test `TestKey` enum first:

```rust
    use super::{RateLimit, RateLimitLayer, RateScope, Scope};
    use crate::body::BufferMode;
    use crate::rate::{LimitDecl, LimitPolicy, RateLimitConfig};
    use crate::{Guarded, HttpError, Service};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_api::{Layer, Timer};
    use oath_adapter_net_http_mock::MockClient;
    use oath_adapter_net_mock::MockTimer;
    use std::collections::HashMap;
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Snapshot, // rate: 2 per 1s
        History,  // concurrency: 1
    }
    impl crate::RateKey for Key {
        fn all() -> &'static [Self] {
            &[Self::Snapshot, Self::History]
        }
    }

    // global 10/s rate; Snapshot 2/s rate; History concurrency 1.
    fn config() -> RateLimitConfig<Key> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 10, per: Duration::from_secs(1), burst: 10 },
            local: HashMap::from([
                (Key::Snapshot, LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 2, per: Duration::from_secs(1), burst: 2 })),
                (Key::History, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
            ]),
        }
    }

    fn layer(timer: MockTimer, max_wait: Duration) -> RateLimitLayer<Key, MockTimer> {
        RateLimitLayer::new(&config(), timer, max_wait).expect("valid config")
    }

    fn req(scope: Scope, key: Option<Key>) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        r.extensions_mut().insert(RateScope { scope, key });
        r
    }

    #[tokio::test]
    async fn a_request_within_budget_passes_and_body_is_guarded() {
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(MockClient::ok("ok"));
        let resp = svc.call(req(Scope::Global, None)).await.expect("passes");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<Guarded<_>> collects transparently
    }

    #[tokio::test]
    async fn local_rate_bucket_throttles_when_drained_and_refills_on_advance() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(MockClient::ok("ok"));
        // Snapshot burst = 2: two pass, third throttles with zero max_wait.
        svc.call(req(Scope::Local, Some(Key::Snapshot))).await.expect("1st");
        svc.call(req(Scope::Local, Some(Key::Snapshot))).await.expect("2nd");
        let err = svc.call(req(Scope::Local, Some(Key::Snapshot))).await.unwrap_err();
        assert_eq!(err, HttpError::Throttled);
        // 2 tokens/sec -> one token after 500ms.
        timer.advance(Duration::from_millis(500));
        svc.call(req(Scope::Local, Some(Key::Snapshot))).await.expect("refilled");
    }

    #[tokio::test]
    async fn none_scope_acquires_nothing() {
        let timer = MockTimer::new();
        let svc = layer(timer, Duration::from_secs(0)).layer(MockClient::ok("ok"));
        for _ in 0..100 {
            svc.call(req(Scope::None, None)).await.expect("unlimited");
        }
    }

    #[tokio::test]
    async fn absent_directive_fails_closed() {
        // A request with no RateScope extension is rejected, never sent
        // (ADR-0034 Amendment #1) — "global only" must be an explicit Scope::Global.
        let leaf = Leaf::ok(b"ok");
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(leaf.clone());
        let err = svc.call(http::Request::new(Bytes::new())).await.unwrap_err();
        assert!(matches!(err, HttpError::Throttled));
        assert_eq!(leaf.calls(), 0, "absent directive must not reach the leaf");
    }

    #[tokio::test]
    async fn concurrency_permit_is_held_until_body_drop() {
        // History concurrency max = 1. First call holds the permit via its
        // (unread) body; a second concurrent acquire must wait, then throttle.
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(MockClient::ok("data"));
        let held = svc.call(req(Scope::Local, Some(Key::History))).await.expect("1st permit");
        let err = svc.call(req(Scope::Local, Some(Key::History))).await.unwrap_err();
        assert_eq!(err, HttpError::Throttled, "permit still held by first body");
        drop(held); // releasing the body frees the permit
        svc.call(req(Scope::Local, Some(Key::History))).await.expect("permit freed");
    }

    #[tokio::test]
    async fn concurrency_waits_within_max_wait_then_succeeds() {
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(30)).layer(MockClient::ok("data"));
        let held = svc.call(req(Scope::Local, Some(Key::History))).await.expect("1st permit");
        // Second acquire blocks on the semaphore; spawn it, free the permit, and
        // it completes within max_wait.
        let svc2 = svc.clone();
        let waiter = tokio::spawn(async move { svc2.call(req(Scope::Local, Some(Key::History))).await });
        tokio::task::yield_now().await;
        drop(held);
        waiter.await.unwrap().expect("acquired after release");
    }
```

> `MockClient` is `Clone` and `RateLimit` derives no `Clone` requirement on `S` beyond what tests use; `svc.clone()` requires `RateLimit<S, K, T>: Clone` — provided in Step 3.

- [ ] **Step 3: Implement the layer**

Add the imports and types to `rate_limit.rs` (above the tests). This is the core of the PR:

```rust
use crate::body::Guarded;
use crate::rate::{LimitDecl, LimitPolicy, RateLimitConfig, validate_concurrency_singleton, validate_coverage};
use crate::{BuildError, HttpError, RateKey, Service};
use async_lock::{Semaphore, SemaphoreGuardArc};
use bytes::Bytes;
use futures_util::future::{Either, select};
use oath_adapter_net_api::Timer;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A refilling token-bucket's mutable state (ADR-0031 §3). Guarded by a
/// `std::sync::Mutex` that is **always released before any `await`**.
struct TokenState {
    tokens: f64,
    last: Instant,
}

/// One endpoint's (or the global) pacing state.
enum Bucket {
    /// A token bucket: `refill_per_sec` tokens/second, capped at `burst`.
    Rate { refill_per_sec: f64, burst: f64, state: Mutex<TokenState> },
    /// A concurrency semaphore with `max` permits.
    Concurrency(Arc<Semaphore>),
}

impl Bucket {
    fn build(policy: LimitPolicy, now: Instant) -> Self {
        match policy {
            LimitPolicy::TokenBucket { rate, per, burst } => Self::Rate {
                refill_per_sec: f64::from(rate) / per.as_secs_f64(),
                burst: f64::from(burst),
                state: Mutex::new(TokenState { tokens: f64::from(burst), last: now }),
            },
            LimitPolicy::Concurrency { max } => {
                Self::Concurrency(Arc::new(Semaphore::new(max as usize)))
            },
        }
    }
}

/// The frozen bucket map — key set fixed at construction, so lookup is lock-free
/// and each bucket owns its own lock (contention scoped to one endpoint).
struct RateState<K> {
    global: Bucket,
    local: HashMap<K, Bucket>,
}

/// The `RateLimit` `Layer` factory: holds the shared, validated bucket state and
/// produces a [`RateLimit`] around any inner service.
pub struct RateLimitLayer<K, T> {
    state: Arc<RateState<K>>,
    timer: T,
    max_wait: Duration,
}

impl<K, T> Clone for RateLimitLayer<K, T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self { state: Arc::clone(&self.state), timer: self.timer.clone(), max_wait: self.max_wait }
    }
}

impl<K, T> fmt::Debug for RateLimitLayer<K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitLayer").field("max_wait", &self.max_wait).finish_non_exhaustive()
    }
}

impl<K, T> RateLimitLayer<K, T> {
    /// Build the pacing layer from a config, validating coverage and the
    /// ≤1-concurrency-permit invariant at construction (a boot failure).
    ///
    /// `max_wait` bounds the whole acquire: an exhausted bucket backpressures up
    /// to this, then the request returns [`HttpError::Throttled`].
    ///
    /// # Errors
    /// Propagates [`validate_coverage`]'s and [`validate_concurrency_singleton`]'s
    /// [`BuildError`].
    pub fn new(cfg: &RateLimitConfig<K>, timer: T, max_wait: Duration) -> Result<Self, BuildError>
    where
        K: RateKey + fmt::Debug,
        T: Timer,
    {
        validate_coverage(cfg)?;
        validate_concurrency_singleton(cfg)?;
        let now = timer.now();
        let global = Bucket::build(cfg.global, now);
        let mut local = HashMap::new();
        for (key, decl) in &cfg.local {
            if let LimitDecl::Policy(policy) = decl {
                local.insert(key.clone(), Bucket::build(*policy, now));
            }
        }
        Ok(Self { state: Arc::new(RateState { global, local }), timer, max_wait })
    }
}

impl<S, K, T> Layer<S> for RateLimitLayer<K, T>
where
    T: Clone,
{
    type Service = RateLimit<S, K, T>;

    fn layer(&self, inner: S) -> RateLimit<S, K, T> {
        RateLimit {
            inner,
            state: Arc::clone(&self.state),
            timer: self.timer.clone(),
            max_wait: self.max_wait,
        }
    }
}

/// The `RateLimit` middleware: paces each request against its buckets, then
/// returns `http::Response<Guarded<B>>` so a concurrency permit rides the body.
pub struct RateLimit<S, K, T> {
    inner: S,
    state: Arc<RateState<K>>,
    timer: T,
    max_wait: Duration,
}

impl<S, K, T> Clone for RateLimit<S, K, T>
where
    S: Clone,
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: Arc::clone(&self.state),
            timer: self.timer.clone(),
            max_wait: self.max_wait,
        }
    }
}

impl<S, K, T> fmt::Debug for RateLimit<S, K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimit").field("max_wait", &self.max_wait).finish_non_exhaustive()
    }
}

impl<S, K, T> RateLimit<S, K, T>
where
    K: RateKey,
    T: Timer,
{
    /// Acquire the buckets `directive` calls for, in the order rate-then-
    /// concurrency (global before local), bounded by a single `max_wait`
    /// deadline. Returns the held concurrency permit (if any) for `Guarded`.
    async fn acquire(&self, directive: &RateScope<K>) -> Result<Option<SemaphoreGuardArc>, HttpError> {
        if matches!(directive.scope, Scope::None) {
            return Ok(None);
        }
        let want_global = matches!(directive.scope, Scope::Global | Scope::Both);
        let want_local = matches!(directive.scope, Scope::Local | Scope::Both);

        // Collect applicable buckets, rate-type first (ADR-0031 §3 acquire order).
        let mut rate: Vec<&Bucket> = Vec::new();
        let mut conc: Vec<&Bucket> = Vec::new();
        let deadline = self.timer.now() + self.max_wait;

        // global first, then local
        if want_global {
            push_bucket(&self.state.global, &mut rate, &mut conc);
        }
        if want_local {
            // Fail-closed: `Local`/`Both` require a present key + local bucket,
            // else the request cannot be paced and must not be sent unthrottled.
            let key = directive.key.as_ref().ok_or(HttpError::Throttled)?;
            let bucket = self.state.local.get(key).ok_or(HttpError::Throttled)?;
            push_bucket(bucket, &mut rate, &mut conc);
        }

        for bucket in rate {
            acquire_rate(bucket, &self.timer, deadline).await?;
        }
        let mut held = None;
        for bucket in conc {
            held = Some(acquire_conc(bucket, &self.timer, deadline).await?);
        }
        Ok(held)
    }
}

/// Route a bucket into the rate-first / concurrency-second acquire lists.
fn push_bucket<'a>(bucket: &'a Bucket, rate: &mut Vec<&'a Bucket>, conc: &mut Vec<&'a Bucket>) {
    match bucket {
        Bucket::Rate { .. } => rate.push(bucket),
        Bucket::Concurrency(_) => conc.push(bucket),
    }
}

/// Consume one rate token, refilling from elapsed time; wait (lock released
/// first) until one accrues, or return `Throttled` if that would breach the
/// deadline.
async fn acquire_rate<T: Timer>(bucket: &Bucket, timer: &T, deadline: Instant) -> Result<(), HttpError> {
    let Bucket::Rate { refill_per_sec, burst, state } = bucket else {
        return Ok(()); // not a rate bucket — nothing to do
    };
    loop {
        let wait = {
            let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
            let now = timer.now();
            let elapsed = now.saturating_duration_since(st.last).as_secs_f64();
            st.tokens = (st.tokens + elapsed * refill_per_sec).min(*burst);
            st.last = now;
            if st.tokens >= 1.0 {
                st.tokens -= 1.0;
                return Ok(());
            }
            Duration::from_secs_f64((1.0 - st.tokens) / refill_per_sec)
        }; // lock dropped here — before any await
        if timer.now() + wait > deadline {
            return Err(HttpError::Throttled);
        }
        timer.sleep(wait).await;
    }
}

/// Acquire a concurrency permit, racing the semaphore against the deadline.
async fn acquire_conc<T: Timer>(bucket: &Bucket, timer: &T, deadline: Instant) -> Result<SemaphoreGuardArc, HttpError> {
    let Bucket::Concurrency(sem) = bucket else {
        return Err(HttpError::Throttled); // unreachable given push_bucket, but total
    };
    let remaining = deadline.saturating_duration_since(timer.now());
    let acquire = sem.acquire_arc();
    let sleep = timer.sleep(remaining);
    let mut acquire = std::pin::pin!(acquire);
    let mut sleep = std::pin::pin!(sleep);
    match select(acquire.as_mut(), sleep.as_mut()).await {
        Either::Left((guard, _)) => Ok(guard),
        Either::Right(((), _)) => Err(HttpError::Throttled),
    }
}

impl<S, K, T, B> Service<http::Request<Bytes>> for RateLimit<S, K, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    K: RateKey,
    T: Timer,
    B: http_body::Body + Send,
{
    type Response = http::Response<Guarded<B>>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            // Absent directive fails closed (ADR-0034 Amendment #1): a forgotten
            // stamp must never fly unpaced or global-only.
            let Some(directive) = req.extensions().get::<RateScope<K>>().cloned() else {
                return Err(HttpError::Throttled);
            };
            let permit = self.acquire(&directive).await?;
            let resp = self.inner.call(req).await?;
            let (parts, body) = resp.into_parts();
            Ok(http::Response::from_parts(parts, Guarded::new(body, permit)))
        }
    }
}
```

Add `use oath_adapter_net_api::Layer;` to the imports (with the `Timer` import), and add the mock dev-dependencies to `Cargo.toml` `[dev-dependencies]`:

```toml
oath-adapter-net-http-mock = { workspace = true }
oath-adapter-net-mock = { workspace = true }
http-body-util = { workspace = true }
```

(`http-body-util` may already be a normal dep; if so, it is usable in tests without a dev entry — only add the two mock crates then.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api rate_limit && just lint`
Expected: PASS, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs crates/adapter/net/http/api/Cargo.toml
git commit -m "feat(net): RateLimit layer — token-bucket + concurrency acquire"
```

---

## Task 4: Fail-closed on runtime coverage gaps + re-exports

**Files:**
- Modify: `crates/adapter/net/http/api/src/rate_limit.rs`

**Interfaces:**
- Consumes: everything from Task 3 — the existing inline test doubles (`Leaf`, `StubBody`) and helpers (`config()`, `layer()`, `req()`, the `Key` enum), NOT `MockClient` (Task 3 established that `MockClient` forms a non-compiling dev-dep cycle, so the tests use inline doubles per the `body.rs`/`auth.rs` house pattern).
- Produces: two fail-closed tests. **The `lib.rs` re-export `pub use rate_limit::{RateLimit, RateLimitLayer, RateScope, Scope};` already landed in Task 3** (pulled forward), so this task does NOT touch `lib.rs`. The fail-closed behavior is already coded in `acquire` (Task 3, the `ok_or(HttpError::Throttled)?` lines); this task pins it with tests.

- [ ] **Step 1: Extend the `Leaf` double to record whether it was called**

The Task-3 `Leaf` returns a canned `http::Response<StubBody>` but does not record calls. To assert "the request never reached the leaf," add a call counter to the existing `Leaf` struct in the `tests` module: an `Arc<std::sync::atomic::AtomicUsize>` field, bumped with `fetch_add(1, Relaxed)` at the top of its `call`, a `fn calls(&self) -> usize` accessor (`load(Relaxed)`), and `#[derive(Clone)]` so a test can hold a handle to the same leaf it wrapped. Keep `Leaf::ok(...)` working (initialize the counter to zero). Mechanical extension — match the double's current field/constructor style.

- [ ] **Step 2: Write the failing tests**

Add to the `rate_limit.rs` `tests` module (reusing `config()`, `layer()`, `req()`, and the extended `Leaf`):

```rust
    // Snapshot has a local bucket; reclassify it GlobalOnly so it has NONE.
    fn config_with_globalonly() -> RateLimitConfig<Key> {
        let mut cfg = config();
        cfg.local.insert(Key::Snapshot, LimitDecl::GlobalOnly); // Snapshot now has NO local bucket
        cfg
    }

    #[tokio::test]
    async fn local_scope_on_a_globalonly_key_fails_closed() {
        let l = RateLimitLayer::new(&config_with_globalonly(), MockTimer::new(), Duration::from_secs(0))
            .expect("valid config");
        let leaf = Leaf::ok(b"ok");
        let svc = l.layer(leaf.clone());
        let err = svc.call(req(Scope::Local, Some(Key::Snapshot))).await.unwrap_err();
        assert!(matches!(err, HttpError::Throttled));
        assert_eq!(leaf.calls(), 0, "must never reach the leaf");
    }

    #[tokio::test]
    async fn local_scope_with_no_key_fails_closed() {
        let leaf = Leaf::ok(b"ok");
        let svc = layer(MockTimer::new(), Duration::from_secs(0)).layer(leaf.clone());
        let err = svc.call(req(Scope::Local, None)).await.unwrap_err();
        assert!(matches!(err, HttpError::Throttled));
        assert_eq!(leaf.calls(), 0, "must never reach the leaf");
    }
```

(Adapt the `b"ok"` literal to `Leaf::ok`'s actual signature from Task 3.)

- [ ] **Step 3: Run to verify they pass (behavior already coded)**

Run: `cargo test -p oath-adapter-net-http-api rate_limit`
Expected: PASS — the `ok_or(HttpError::Throttled)?` lines in `acquire` already implement this. If either test **fails** because the leaf WAS called (`calls() == 1`), the acquire order is wrong — `call` must `self.acquire(...).await?` before `self.inner.call(...)`.

- [ ] **Step 4: Full-crate check**

Run: `just check && cargo test -p oath-adapter-net-http-api && just lint`
Expected: PASS, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "test(net): RateLimit fail-closed coverage-gap tests"
```

---

## Task 5: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Add to ADR-0034's **Amendments (2026-07-04)** section (append-only) a bullet:

```markdown
- **`RateLimit` layer (Slice 1 PR 1).** `LimitPolicy::TokenBucket` gains a
  `per: Duration` so IBKR's sub-1/second limits (`1/5s`, `1/min`, `1/15min`) are
  expressible with integer parameters; `validate_coverage` rejects a zero period.
  The per-request directive ships as `RateScope<K>` (renamed from §3's
  `RateLimit<K>` sketch, which collided with the layer name). The
  ≤1-concurrency-permit invariant (`Guarded` holds one) is enforced at
  construction by `BuildError::MultipleConcurrency` / `validate_concurrency_singleton`.
```

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added` (after the PR 4 boot-coverage entry):

```markdown
- `oath-adapter-net-http-api` `RateLimit` resilience layer (Slice 1) — the
  `RateLimit<S, K, T>` service + `RateLimitLayer<K, T>` factory (`net-api::Layer`):
  proactive per-endpoint pacing (token-bucket + concurrency policies) built from a
  validated `RateLimitConfig`, driven by `net-api::Timer` (mockable clock). Adds the
  `RateScope`/`Scope` per-request directive (absent → fails closed; `None` → opt-out;
  a runtime coverage gap fails closed as `Throttled`, never sent). `LimitPolicy::
  TokenBucket` gains `per: Duration` for sub-1/second venue limits, and the
  ≤1-concurrency-permit invariant is a boot check (`BuildError::MultipleConcurrency`).
  (ADR-0031 §3–4.)
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete — `futures-util` is an existing workspace dep, so `deny`/`machete` are unaffected).

- [ ] **Step 4: Commit, push, PR**

```bash
git add CHANGELOG.md docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md
git commit -m "docs(net): record RateLimit layer amendments (ADR-0034) + changelog"
git push -u origin feat/net-http-ratelimit
gh pr create \
  --title "feat(net): RateLimit resilience layer (Slice 1, PR 1)" \
  --body "Closes #<N>

Slice 1 **PR 1** of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-ratelimit-layer-design.md; ADR-0031 §3-4).

- **\`RateLimit<S, K, T>\`** + **\`RateLimitLayer<K, T>\`** (\`net-api::Layer\`) — proactive pacing so the stack never hits IBKR's 429 penalty box. Rate tokens acquired before concurrency permits (no-starvation), lock released before every await, a single \`max_wait\` deadline bounds the whole acquire.
- **\`RateScope<K>\`/\`Scope\`** per-request directive — absent → fails **closed** (\`Throttled\`, never sent; ADR-0034 Amend #1); \`None\` → explicit opt-out; a \`Local\`/\`Both\` directive on a bucketless or keyless request also fails closed.
- Always returns \`http::Response<Guarded<B>>\` — a concurrency permit rides the body to stream-end/drop.
- **\`LimitPolicy::TokenBucket\`** gains **\`per: Duration\`** so IBKR's \`1/5s\`/\`1/min\`/\`1/15min\` are expressible; **\`BuildError::MultipleConcurrency\`** enforces the ≤1-concurrency-permit invariant at boot.

Runtime-neutral: generic over \`net-api::Timer\`, \`async-lock\` semaphore, \`futures-util\` race — no \`tokio\`/\`hyper\`. MockTimer-driven tests.

Next: **Slice 1 PR 2** — the \`Timeout\` layer.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `RateLimit<S, K, T>` + `RateLimitLayer<K, T>` (`Layer`) — Task 3. ✅
- `RateState<K>` frozen bucket map, per-bucket `Mutex`/`Semaphore` — Task 3. ✅
- `RateScope<K>` + `Scope`, absent→fail-closed (ADR-0034 Amend #1), `None`→nothing — Tasks 2, 3. ✅
- Test coverage note: a sub-1/second runtime pacing test (`rate:1, per:5s`) is added in the final hygiene pass. The explicit acquire-order / no-starvation runtime test is **deferred**: the order is coded and statically verified (final review), and a deterministic MockTimer-driven no-starvation assertion needs fragile multi-task scheduling — tracked for a follow-up rather than shipped flaky.
- Fail-closed on runtime coverage gap — Task 3 (coded) + Task 4 (tests). ✅
- Acquire order rate-before-concurrency, global-first, lock-released-before-await, single `max_wait` deadline — Task 3 (`acquire`/`acquire_rate`/`acquire_conc`). ✅
- Token refill `min(burst, tokens + elapsed × rate/per)` — Task 3 (`acquire_rate`). ✅
- Permit lifetime → `Guarded` (rate ZST / concurrency moved into body) — Task 3 (`call`). ✅
- `LimitPolicy` `per` amendment + `validate` — Task 1. ✅
- ≤1-concurrency-permit invariant documented + enforced (`MultipleConcurrency`) — Task 1 + ADR note (Task 5). ✅
- MockTimer-driven tests (refill, sub-1/s, max_wait, concurrency lifetime, fail-closed, None, absent) — Tasks 3, 4. ✅
- Deferred (correctly absent): `Timeout`/`Retry`/`CircuitBreaker`/`Tracing`, `stack()`/`build()`, `CircuitOpen`, tokio `Timer`, multiple concurrency permits. ✅

**Placeholder scan:** None — every step carries actual code or an actual command with expected output.

**Type consistency:**
- `LimitPolicy::TokenBucket { rate, per, burst }` — identical in Task 1's def, `validate`, and every Task 1/3 literal.
- `Scope::{None, Global, Local, Both}` and `RateScope { scope, key }` — identical across Tasks 2, 3, 4.
- `RateLimitLayer::new(&RateLimitConfig<K>, T, Duration) -> Result<Self, BuildError>` — matches the `Interfaces` block and every test call.
- `RateLimit` `Service` impl: inner `Response = http::Response<B>` → `Response = http::Response<Guarded<B>>` — matches `MockClient::Response = http::Response<MockBody>` (so `B = MockBody`) and `Guarded::new(B, Option<SemaphoreGuardArc>)`.
- `acquire → Option<SemaphoreGuardArc>`, threaded into `Guarded::new` — consistent.
- `validate_concurrency_singleton` / `BuildError::MultipleConcurrency` — consistent between Task 1 def, `new` call (Task 3), and tests.

**Known risks to watch during impl:**
- `select` needs `Unpin` futures — `std::pin::pin!` both before `select` (shown). `futures_util::future::select` polls the first arg first, so an immediately-available permit at a zero deadline is still taken (permit before timer).
- Clippy `cast_possible_truncation` on `max as usize` (Semaphore) — add `#[allow(clippy::cast_possible_truncation)]` on `Bucket::build` if it fires, or `usize::try_from(max).unwrap_or(usize::MAX)`.
- Holding the `std::sync::MutexGuard` across `.await` would make the future non-`Send`; the `wait` block drops it before `timer.sleep().await` — keep that scope.
- `Guarded` releases an already-ended (buffered) body's permit on construction, so the "held until drop" test uses an **unread** body (never collected) to keep the permit live.
- `f64::from(rate) / per.as_secs_f64()` — `per` non-zero is guaranteed by `validate` (Task 1), so no divide-by-zero reaches `Bucket::build`.
