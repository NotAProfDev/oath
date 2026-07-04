# net-http `Retry` Layer (Slice 1, PR 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `Retry<S, T>` HTTP middleware layer that re-issues an **explicitly-eligible** request on a **transient** failure (`HttpError::{Timeout, Connection}` or a `5xx` response) with capped-exponential, full-jitter backoff up to a configured attempt count — and passes everything else (a `POST` with no opt-in, a 429, a 4xx, an `Auth` error) through unretried.

**Architecture:** A `Timer`-generic, runtime-neutral `Service` wrapper in `oath-adapter-net-http-api`. It reads a `Retryable` marker request extension (absent → never retried, fail-safe), then loops: clone the request, call inner, and if the outcome is a transient error or a 5xx status *and* attempts remain, **drop** the prior response (releasing any `Guarded` permit), sleep a full-jitter backoff (`delay ∈ [0, min(cap, base·2ⁿ⁻¹)]`) drawn from an internal seeded `SplitMix64`, and retry. **Body-transparent** — `http::Response<B>` is returned untouched (same `B`, no `B: Body` bound). `Auth`/`RateLimit` re-run for free each attempt because they sit *inside* `Retry`.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `http`/`bytes`, `std::time::Duration`, `std::num::NonZeroU32`, `std::sync::atomic::AtomicU64`, `net-api::{Timer, ErrorKind, HasErrorKind, Layer}`. Tests use inline service doubles + `MockTimer` (`oath-adapter-net-mock`) + the production `Guarded` (permit test), driven on `tokio` (dev-only). **No new dependency.**

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result` or use total combinators (`checked_mul(...).unwrap_or(cap)`, `try_from(...).unwrap_or(...)`). Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations` — hand-impl where a derive would demand `Debug`/`Clone` on `S`), `const fn` where `missing_const_for_fn` asks.
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde`/`rand` in non-dev deps. **This PR adds no dependency** (`http`/`bytes`/`http-body`/`async-lock` are crate deps; `oath-adapter-net-mock` + `tokio` are dev-deps — all present since #76/#78), so `cargo-deny`/`machete` are unaffected.
- **net-http-api tests must NOT dev-depend on `oath-adapter-net-http-mock` (`MockClient`)** — it normal-depends on this crate, so the dev-dep closes a cycle that recompiles a second, non-unifying copy of `net-http-api` (E0599: `MockClient` does not satisfy *this* crate's `Service`). Use **inline** service doubles + `oath-adapter-net-mock`'s `MockTimer`, exactly as `rate_limit.rs`/`timeout.rs`/`body.rs` do.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-04-net-http-retry-layer-design.md](../specs/2026-07-04-net-http-retry-layer-design.md), governed by [ADR-0031 §2](../../adr/0031-http-resilience-venue-pacing.md) and [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 1, PR 3** — the third of the resilience-layer PRs (RateLimit #76, Timeout #78 landed; then Retry, CircuitBreaker, Tracing).

## File Structure

- `crates/adapter/net/http/api/src/retry.rs` — **new** (Tasks 1–3). `Retryable`, `RetryConfig`, the internal `SplitMix64`, `RetryLayer<T>`, `Retry<S, T>`, the `Layer`/`Service` impls, the `is_transient`/`backoff_ceiling` helpers, and their tests.
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Tasks 1, 3). `pub mod retry;` + re-exports + module-doc bullet.
- `docs/adr/0034-...md`, `CHANGELOG.md` — **modify** (Task 4).

No `Cargo.toml` change. Each task is one or more commits; the tasks together are one PR/issue.

---

## Setup: issue (worktree already exists)

> The isolated worktree **already exists** at `.claude/worktrees/net-http-retry` (branch `feat/net-http-retry`, branched off `main` = `de2e5e4`, which carries #76/#78/#80). All tasks run inside it. Only the GitHub issue remains to be created.

- [ ] **Create the issue**

```bash
gh issue create \
  --title "feat(net): Retry resilience layer (Slice 1, PR 3)" \
  --label enhancement \
  --body "Slice 1 PR 3 of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-retry-layer-design.md; ADR-0031 §2).

- \`Retry<S, T>\` + \`RetryLayer<T>\` (impl \`net-api::Layer\`): order-safe retry — re-issues an explicitly-eligible request on a transient failure (\`HttpError::{Timeout, Connection}\`) or a 5xx response, with capped-exponential full-jitter backoff up to \`max_attempts\`
- \`Retryable\` marker request extension — explicit-only, fail-safe: absent -> never retried (tightens ADR-0031 §2's idempotent-method default; never duplicates a POST)
- Never retries a 429/other 4xx/\`Auth\`; returns the last outcome verbatim on exhaustion; body-transparent (drops a superseded response, releasing its \`Guarded\` permit)
- Jitter via an internal seeded \`SplitMix64\` — no new dependency, no injected \`Jitter\` generic"
```

Note the issue number `#<N>` for the PR body.

---

## Task 1: `Retryable` marker + `RetryConfig` + module scaffold

**Files:**
- Create: `crates/adapter/net/http/api/src/retry.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: nothing (only `std::num::NonZeroU32`, `std::time::Duration`).
- Produces:
  - `oath_adapter_net_http_api::Retryable` — `struct Retryable;` (`Debug`, `Clone`, `Copy`), a marker `http::Request` extension whose presence opts a request into retry.
  - `oath_adapter_net_http_api::RetryConfig` — `struct { max_attempts: NonZeroU32, base: Duration, cap: Duration, seed: u64 }` (`Debug`, `Clone`, `Copy`).
  - Tasks 2–3 add the `SplitMix64` PRNG and the `RetryLayer`/`Retry` layer to this module.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/retry.rs` with the module doc + the directive/config types + the round-trip test:

```rust
//! The `Retry` resilience layer (ADR-0031 §2): order-safe retry.
//!
//! Re-issues an **explicitly-eligible** request (a [`Retryable`] marker
//! extension — **absent → never retried**, so a forgotten stamp never
//! duplicates a `POST`) on a **transient** failure (`HttpError::{Timeout,
//! Connection}`) or a `5xx` response, with capped-exponential **full-jitter**
//! backoff up to [`RetryConfig::max_attempts`]. A 429/other 4xx, an `Auth`
//! error, or an `Other` error is **never** retried; on exhaustion the last
//! outcome is returned verbatim. **Body-transparent:** the response body is
//! returned untouched (a superseded response is dropped, releasing any
//! `Guarded` permit). `Auth`/`RateLimit` re-run per attempt because they sit
//! *inside* `Retry`. Runtime-neutral: generic over
//! [`Timer`](oath_adapter_net_api::Timer), jitter via an internal seeded
//! `SplitMix64` (no `rand` dependency).

use std::num::NonZeroU32;
use std::time::Duration;

/// A marker `http::Request` extension: its **presence** opts the request into
/// retry (ADR-0031 §2). `Copy` so it survives the per-attempt request clone.
///
/// Eligibility is **explicit-only and fail-safe**: an **absent** marker means
/// the request is sent exactly once and its outcome returned verbatim — a
/// forgotten stamp disables retry, it never duplicates a non-idempotent `POST`.
/// This tightens ADR-0031 §2's "retry idempotent *methods*" default into
/// adapter-stamped intent, the same structural-safety move ADR-0034 Amendment #1
/// made for `RateScope` (see ADR-0034 Amendment #8).
#[derive(Debug, Clone, Copy)]
pub struct Retryable;

/// The `Retry` layer's schedule, as plain `Copy` data.
///
/// `max_attempts` is the **total** number of sends (retries = `max_attempts − 1`);
/// `NonZeroU32` makes "at least one send" a type invariant, so
/// [`RetryLayer::new`](crate::RetryLayer) needs no `Result`. Backoff before the
/// `n`-th retry draws a full-jitter delay from `[0, min(cap, base·2ⁿ⁻¹)]`; `seed`
/// seeds the jitter PRNG (varied per process in production, fixed in tests).
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Total sends allowed for one logical request (retries = this − 1).
    pub max_attempts: NonZeroU32,
    /// The first backoff ceiling — the `n = 1` retry draws its delay from `[0, base]`.
    pub base: Duration,
    /// The exponential-ceiling clamp — no backoff ceiling exceeds this.
    pub cap: Duration,
    /// The jitter PRNG seed (deterministic given seed + draw order).
    pub seed: u64,
}

#[cfg(test)]
mod tests {
    use super::Retryable;

    #[test]
    fn retryable_marker_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut().insert(Retryable);
        assert!(
            req.extensions().get::<Retryable>().is_some(),
            "marker present → eligible"
        );

        let bare = http::Request::new(bytes::Bytes::new());
        assert!(
            bare.extensions().get::<Retryable>().is_none(),
            "absent marker → not eligible (fail-safe)"
        );
    }
}
```

In `lib.rs`, add the module-doc bullet (after the `rate_limit` bullet, before the `timeout` bullet), the `pub mod`, and the re-export (keep alphabetical ordering — `retry` sits after `rate_limit`, before `service`):

Module-doc bullet (insert between the `rate_limit` bullet and the `timeout` bullet):

```rust
//! - [`retry`] — the `Retry` layer, its `RetryLayer` factory, and the
//!   `Retryable`/`RetryConfig` retry directive + schedule
```

Module declaration (insert after `pub mod rate_limit;`):

```rust
pub mod retry;
```

Re-export (insert after the `rate_limit::{…}` re-export block, before `pub use service::Service;`):

```rust
pub use retry::{RetryConfig, Retryable};
```

(Task 3 extends this to `pub use retry::{Retry, RetryConfig, RetryLayer, Retryable};`.)

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — before adding the code, unresolved module `retry` / `cannot find type Retryable`. After adding the code it should compile; the step confirms the wiring, so if it already passes, proceed.

- [ ] **Step 3: Confirm the test passes**

Run: `cargo test -p oath-adapter-net-http-api retry && just lint`
Expected: PASS, warning-free. (`Retryable`/`RetryConfig` are fully defined in Step 1; this task has no separate implementation step.)

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): Retryable marker + RetryConfig retry directive/schedule"
```

---

## Task 2: internal `SplitMix64` full-jitter PRNG

**Files:**
- Modify: `crates/adapter/net/http/api/src/retry.rs`

**Interfaces:**
- Consumes: nothing (only `std`).
- Produces (crate-private — **not** re-exported):
  - `SplitMix64` — `pub(crate) struct` with `pub(crate) const fn new(seed: u64) -> Self`, `Clone`, `Debug`, and `pub(crate) fn duration_in(&self, ceil: Duration) -> Duration` returning a uniform `Duration` in `[0, ceil]`. Interior-mutable (`AtomicU64` state) so `duration_in` takes `&self` and holds no lock across an `await`. Task 3's `Retry` owns one.

- [ ] **Step 1: Write the failing tests**

Append a **new** `#[cfg(test)]` test module for the PRNG below the existing `tests` module in `retry.rs` (two test modules in one file is fine; name this one `rng_tests`):

```rust
#[cfg(test)]
mod rng_tests {
    use super::SplitMix64;
    use std::time::Duration;

    #[test]
    fn same_seed_reproduces_the_same_sequence() {
        let a = SplitMix64::new(0x1234_5678);
        let b = SplitMix64::new(0x1234_5678);
        let ceil = Duration::from_millis(1000);
        for _ in 0..64 {
            assert_eq!(a.duration_in(ceil), b.duration_in(ceil), "seeded PRNG is deterministic");
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let a = SplitMix64::new(1);
        let b = SplitMix64::new(2);
        let ceil = Duration::from_millis(1000);
        // Over many draws the two sequences must differ somewhere (not lockstep).
        let differs = (0..64).any(|_| a.duration_in(ceil) != b.duration_in(ceil));
        assert!(differs, "different seeds must not produce identical sequences");
    }

    #[test]
    fn draws_never_exceed_the_ceiling() {
        let rng = SplitMix64::new(42);
        let ceil = Duration::from_micros(500);
        for _ in 0..10_000 {
            assert!(rng.duration_in(ceil) <= ceil, "full jitter stays within [0, ceil]");
        }
    }

    #[test]
    fn zero_ceiling_yields_zero() {
        let rng = SplitMix64::new(7);
        assert_eq!(rng.duration_in(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn clone_snapshots_state_independently() {
        let a = SplitMix64::new(99);
        let ceil = Duration::from_millis(50);
        let _ = a.duration_in(ceil); // advance `a`
        let b = a.clone();           // `b` continues from `a`'s current state
        assert_eq!(a.duration_in(ceil), b.duration_in(ceil), "clone snapshots the state");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type SplitMix64 in module retry`.

- [ ] **Step 3: Implement the PRNG**

Insert between the `RetryConfig` definition and the first `tests` module in `retry.rs`. Extend the top-of-file `use` block to add the atomics import:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
```

Add the type:

```rust
/// A small [SplitMix64](https://prng.di.unimi.it/splitmix64.c) PRNG for backoff
/// jitter — deterministic given its seed and draw order.
///
/// Lock-free: the 64-bit state advances by the SplitMix64 step constant via
/// `AtomicU64::fetch_add`, so `duration_in` takes `&self` and holds **no** lock
/// across the backoff `await` (the future stays `Send`). Not cryptographic —
/// full-jitter backoff needs a spread, not uniformity guarantees.
#[derive(Debug)]
pub(crate) struct SplitMix64 {
    state: AtomicU64,
}

impl Clone for SplitMix64 {
    fn clone(&self) -> Self {
        // Snapshot the current state — a cloned service continues the sequence.
        Self { state: AtomicU64::new(self.state.load(Ordering::Relaxed)) }
    }
}

impl SplitMix64 {
    /// The SplitMix64 stepping constant (fractional bits of the golden ratio).
    const STEP: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Seed the generator.
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: AtomicU64::new(seed) }
    }

    /// Advance the state and return the next 64-bit draw (SplitMix64 finalizer).
    fn next_u64(&self) -> u64 {
        // `fetch_add` returns the *old* state; add STEP to get the new one — so a
        // fresh generator's first draw finalizes `seed + STEP`, as the reference does.
        let mut z = self.state.fetch_add(Self::STEP, Ordering::Relaxed).wrapping_add(Self::STEP);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform `Duration` in `[0, ceil]` — one full-jitter sample.
    pub(crate) fn duration_in(&self, ceil: Duration) -> Duration {
        // `ceil` comes from `backoff_ceiling` (≤ `cap`); clamp its nanos into u64
        // (a `cap` above ~584 years is not a real config — clamp, don't panic).
        let ceil_nanos = u64::try_from(ceil.as_nanos()).unwrap_or(u64::MAX);
        // Uniform and inclusive over [0, ceil_nanos]. `checked_add(1)` is the modulus
        // for every representable ceiling; at the u64::MAX edge there is no MAX+1
        // modulus, but the bare u64 draw already spans [0, u64::MAX] inclusively.
        // (`ceil_nanos == 0` → `% 1` → ZERO.) Modulo bias is irrelevant for backoff.
        let pick = ceil_nanos
            .checked_add(1)
            .map_or_else(|| self.next_u64(), |modulus| self.next_u64() % modulus);
        Duration::from_nanos(pick)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api retry && just lint`
Expected: PASS, warning-free.

> Known risks:
> - `Duration::from_nanos` takes `u64`; `self.next_u64() % modulus ≤ ceil_nanos ≤ u64::MAX`, so it never overflows and the result is `≤ ceil`.
> - `u64::try_from(ceil.as_nanos()).unwrap_or(u64::MAX)` and `ceil_nanos.checked_add(1).unwrap_or(u64::MAX)` are total combinators (no `.unwrap()`/panic in non-test code).
> - If clippy `unreadable_literal` fires on the hex constants, they already use `_` grouping; if `missing_const_for_fn` asks for `const` on `next_u64`/`duration_in`, leave them non-`const` (they read the atomic — not const-eval-able).

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs
git commit -m "feat(net): internal SplitMix64 full-jitter PRNG for Retry backoff"
```

---

## Task 3: `Retry` layer — eligibility, retry decision, backoff

**Files:**
- Modify: `crates/adapter/net/http/api/src/retry.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `Retryable`, `RetryConfig`, `SplitMix64` (Tasks 1–2); `HttpError`, `Service`, `Guarded` (crate); `ErrorKind`, `HasErrorKind`, `Layer`, `Timer` (`oath_adapter_net_api`).
- Produces:
  - `oath_adapter_net_http_api::RetryLayer<T>` — `impl Layer<S>` factory; `pub fn new(cfg: RetryConfig, timer: T) -> Self` (**infallible**).
  - `oath_adapter_net_http_api::Retry<S, T>` — the wrapping `Service`; for an inner `S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync` and `T: Timer`, it is `Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>` (body-transparent — same `B`, no `B: Body` bound).

- [ ] **Step 1: Write the failing tests**

Replace the `use super::Retryable;` line in the **first** `tests` module with the imports + inline doubles + tests below (the `rng_tests` module from Task 2 stays untouched):

```rust
    use super::{Retry, RetryConfig, RetryLayer, Retryable};
    use crate::{Guarded, HttpError, Service};
    use async_lock::Semaphore;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use oath_adapter_net_api::{ErrorKind, Layer, Timer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    // A canned one-frame response body (`Data = Bytes`, `Error = HttpError`).
    // `Debug` so `Result::unwrap_err` can render an unexpected `Ok`.
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(body: &'static [u8]) -> Self {
            Self { data: Some(Bytes::from_static(body)) }
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
            self.data.as_ref().map_or_else(
                || SizeHint::with_exact(0),
                |d| SizeHint::with_exact(d.len() as u64),
            )
        }
    }

    // One scripted outcome per attempt. `Copy` so the leaf can read it by index.
    #[derive(Clone, Copy)]
    enum Step {
        Err(ErrorKind),
        Status(u16),
    }

    fn err_of(kind: ErrorKind) -> HttpError {
        match kind {
            ErrorKind::Timeout => HttpError::Timeout,
            ErrorKind::Connection => HttpError::connection("reset"),
            ErrorKind::Throttled => HttpError::Throttled,
            ErrorKind::Auth => HttpError::auth("expired"),
            _ => HttpError::other("boom"),
        }
    }

    // An inline leaf yielding a scripted sequence of outcomes, counting calls.
    // Once the script is exhausted it repeats the last step (so a one-element
    // `[Err(Connection)]` models an always-failing endpoint). Inline (not
    // `MockClient`) to avoid the net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
    }
    impl ScriptLeaf {
        fn new(steps: Vec<Step>) -> Self {
            Self { steps: Arc::new(steps), calls: Arc::new(AtomicUsize::new(0)) }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self.steps.get(i).copied().unwrap_or_else(|| *self.steps.last().unwrap());
            async move {
                match step {
                    Step::Err(kind) => Err(err_of(kind)),
                    Step::Status(code) => {
                        let mut resp = http::Response::new(StubBody::new(b"body"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    }
                }
            }
        }
    }

    // An inline leaf whose FIRST response is a 5xx whose body holds a real
    // `Guarded` concurrency permit (max = 1); later responses release it. If
    // `Retry` did not DROP the prior response before retrying, the second
    // attempt's `acquire_arc().await` would deadlock — so a passing test proves
    // drop-before-retry.
    #[derive(Clone)]
    struct PermitLeaf {
        sem: Arc<Semaphore>,
        calls: Arc<AtomicUsize>,
    }
    impl PermitLeaf {
        fn new() -> Self {
            Self { sem: Arc::new(Semaphore::new(1)), calls: Arc::new(AtomicUsize::new(0)) }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for PermitLeaf {
        type Response = http::Response<Guarded<StubBody>>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let sem = self.sem.clone();
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            async move {
                let permit = sem.acquire_arc().await; // deadlocks if the prior permit was never dropped
                if i == 0 {
                    let mut resp = http::Response::new(Guarded::new(StubBody::new(b"err"), Some(permit)));
                    *resp.status_mut() = http::StatusCode::from_u16(503).unwrap();
                    Ok(resp)
                } else {
                    drop(permit); // release immediately; the success body holds nothing
                    Ok(http::Response::new(Guarded::new(StubBody::new(b"ok"), None)))
                }
            }
        }
    }

    fn cfg(max_attempts: u32, base: Duration, cap: Duration) -> RetryConfig {
        RetryConfig {
            max_attempts: NonZeroU32::new(max_attempts).unwrap(),
            base,
            cap,
            seed: 0x0BAD_F00D,
        }
    }

    fn req(eligible: bool) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        if eligible {
            r.extensions_mut().insert(Retryable);
        }
        r
    }

    // Drive a spawned retry loop to completion: yield so the task parks at each
    // backoff `sleep`, then advance past the (jittered) delay. `rounds` ≥ the
    // number of backoffs; extra advances after completion are harmless.
    async fn drain<F>(timer: &MockTimer, waiter: &tokio::task::JoinHandle<F>, rounds: u32, cap: Duration)
    where
        F: Send + 'static,
    {
        let _ = waiter; // handle awaited by the caller; we only pump the clock here
        for _ in 0..rounds {
            tokio::task::yield_now().await;
            timer.advance(cap);
        }
    }

    #[tokio::test]
    async fn not_eligible_sends_once_even_on_a_transient_error() {
        // No `Retryable` marker → the fail-safe default: one send, error verbatim.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, Duration::from_millis(1), Duration::from_millis(1)), MockTimer::new())
            .layer(leaf.clone());
        let err = svc.call(req(false)).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)));
        assert_eq!(leaf.calls(), 1, "not eligible → never retried");
    }

    #[tokio::test]
    async fn eligible_transient_error_retries_then_succeeds() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, &waiter, 3, cap).await;
        let resp = waiter.await.unwrap().expect("retry succeeds on the 2nd attempt");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[tokio::test]
    async fn eligible_5xx_is_retried() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = ScriptLeaf::new(vec![Step::Status(503), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, &waiter, 3, cap).await;
        let resp = waiter.await.unwrap().expect("503 retried → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[tokio::test]
    async fn status_429_is_never_retried() {
        // 429 is a 4xx, not a 5xx — terminal even though eligible (ADR-0031 §2).
        let leaf = ScriptLeaf::new(vec![Step::Status(429), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, Duration::from_millis(1), Duration::from_millis(1)), MockTimer::new())
            .layer(leaf.clone());
        let resp = svc.call(req(true)).await.expect("429 returned as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(leaf.calls(), 1, "429 never retried");
    }

    #[tokio::test]
    async fn client_4xx_is_never_retried() {
        let leaf = ScriptLeaf::new(vec![Step::Status(400)]);
        let svc = RetryLayer::new(cfg(3, Duration::from_millis(1), Duration::from_millis(1)), MockTimer::new())
            .layer(leaf.clone());
        let resp = svc.call(req(true)).await.expect("400 returned as Ok");
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        assert_eq!(leaf.calls(), 1);
    }

    #[tokio::test]
    async fn throttled_and_auth_errors_are_never_retried() {
        for kind in [ErrorKind::Throttled, ErrorKind::Auth] {
            let leaf = ScriptLeaf::new(vec![Step::Err(kind), Step::Status(200)]);
            let svc = RetryLayer::new(cfg(3, Duration::from_millis(1), Duration::from_millis(1)), MockTimer::new())
                .layer(leaf.clone());
            let err = svc.call(req(true)).await.unwrap_err();
            assert!(matches!(err, HttpError::Throttled | HttpError::Auth(_)));
            assert_eq!(leaf.calls(), 1, "{kind:?} is terminal, never retried");
        }
    }

    #[tokio::test]
    async fn attempts_exhausted_returns_the_last_outcome_verbatim() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        // Always Connection (one-element script repeats); max_attempts = 3.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, &waiter, 4, cap).await; // 2 backoffs between 3 attempts (+slack)
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)), "the real error, not a synthesized one");
        assert_eq!(leaf.calls(), 3, "exactly max_attempts sends");
    }

    #[tokio::test]
    async fn prior_response_permit_is_released_before_the_retry() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = PermitLeaf::new(); // 5xx holding a permit, then 200
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, &waiter, 3, cap).await;
        // If `Retry` did not drop the 503 (releasing its Guarded permit) before the
        // 2nd attempt, this `await` would hang on the leaf's `acquire_arc`.
        let resp = waiter.await.unwrap().expect("permit freed → 2nd attempt acquires and succeeds");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Retry`/`RetryLayer` in module `retry`.

- [ ] **Step 3: Implement the layer**

Insert the imports + types between the `SplitMix64` impl (Task 2) and the first `tests` module in `retry.rs`. Extend the top-of-file `use` block:

```rust
use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
```

(The `use std::num::NonZeroU32; use std::time::Duration;` and `use std::sync::atomic::{AtomicU64, Ordering};` from Tasks 1–2 stay; keep a single copy of each.)

Add below the `SplitMix64` impl:

```rust
/// The `Retry` [`Layer`] factory: holds the schedule + clock and produces a
/// [`Retry`] around any inner service.
pub struct RetryLayer<T> {
    cfg: RetryConfig,
    timer: T,
}

impl<T> RetryLayer<T> {
    /// Build the layer from a schedule and a [`Timer`] clock.
    ///
    /// **Infallible** — `RetryConfig::max_attempts` is `NonZeroU32` (≥ 1 send is a
    /// type invariant) and `cap < base` is harmless (the ceiling just never grows
    /// past `cap`), so there is nothing to validate (contrast `RateLimitLayer::new`).
    #[must_use]
    pub const fn new(cfg: RetryConfig, timer: T) -> Self {
        Self { cfg, timer }
    }
}

impl<T: Clone> Clone for RetryLayer<T> {
    fn clone(&self) -> Self {
        Self { cfg: self.cfg, timer: self.timer.clone() }
    }
}

impl<T> fmt::Debug for RetryLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryLayer").field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for RetryLayer<T> {
    type Service = Retry<S, T>;

    fn layer(&self, inner: S) -> Retry<S, T> {
        Retry {
            inner,
            cfg: self.cfg,
            timer: self.timer.clone(),
            rng: SplitMix64::new(self.cfg.seed),
        }
    }
}

/// The `Retry` middleware: re-issues an eligible request on a transient failure
/// or a 5xx response, with capped-exponential full-jitter backoff. Body-
/// transparent — the inner `http::Response<B>` is returned untouched.
pub struct Retry<S, T> {
    inner: S,
    cfg: RetryConfig,
    timer: T,
    rng: SplitMix64,
}

impl<S: Clone, T: Clone> Clone for Retry<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cfg: self.cfg,
            timer: self.timer.clone(),
            rng: self.rng.clone(),
        }
    }
}

impl<S, T> fmt::Debug for Retry<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Retry").field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

/// Transient (worth retrying) error kinds — a dropped/timed-out send, not a
/// server verdict. `Throttled`/`Auth`/`Client`/`Server`/`Unknown` are terminal.
const fn is_transient(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Timeout | ErrorKind::Connection)
}

/// The full-jitter ceiling before the retry that follows a **1-based** `attempt`:
/// `min(cap, base · 2^(attempt-1))`, saturating — no `Duration` overflow reaches
/// the caller (`checked_mul` → `cap`), no shift overflow (`shift` capped at 31).
fn backoff_ceiling(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(31);
    let factor = 1u32 << shift;
    base.checked_mul(factor).unwrap_or(cap).min(cap)
}

impl<S, T, B> Service<http::Request<Bytes>> for Retry<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
{
    type Response = http::Response<B>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let eligible = req.extensions().get::<Retryable>().is_some();
            let max = self.cfg.max_attempts.get();
            let mut attempt: u32 = 1;
            loop {
                // Whole-request clone per attempt: `http::Extensions` requires
                // `Clone` on insert, so `Request<Bytes>` is `Clone` (Bytes is a
                // cheap refcount bump; the directives ride along). `Auth`/`RateLimit`
                // re-run inside this call, so credentials/budget refresh for free.
                let outcome = self.inner.call(req.clone()).await;
                let retry = eligible
                    && attempt < max
                    && match &outcome {
                        Err(e) => is_transient(e.kind()),
                        Ok(resp) => resp.status().is_server_error(), // 5xx only; 429 is 4xx
                    };
                if !retry {
                    return outcome; // success, non-retryable outcome, or attempts exhausted
                }
                drop(outcome); // release the prior response's Guarded permit before waiting
                let ceil = backoff_ceiling(self.cfg.base, self.cfg.cap, attempt);
                self.timer.sleep(self.rng.duration_in(ceil)).await;
                attempt += 1;
            }
        }
    }
}
```

In `lib.rs`, extend the Task 1 re-export:

```rust
pub use retry::{Retry, RetryConfig, RetryLayer, Retryable};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api retry && just lint`
Expected: PASS, warning-free.

> Known risks (from the spec's implementation notes):
> - **`req.clone()` requires `http::Request<Bytes>: Clone`** — it holds because `http::Extensions::insert` bounds `T: Clone`, so every stamped directive (`Retryable`/`RateScope`/`RequestTimeout`/`BufferMode`) is `Clone` and `Extensions: Clone`. If a build errors here, an extension type lost its `Clone` — fix that type, not `Retry`.
> - **`S: Sync`** because the returned `Send` future borrows `&self` (`&S: Send ⇒ S: Sync`; `T: Sync` via `Timer`, `SplitMix64`/`AtomicU64` are `Sync`). Same bound `RateLimit`/`Timeout` carry.
> - **`B` carries no `http_body::Body` bound** — `Retry` only reads `status()` (from parts) and drops or returns the response; it never polls the body. Stays fully generic, composing with `RateLimit`'s `Guarded<B>` output.
> - **Backoff timing is *not* asserted "before advance"** — full jitter admits a zero draw, so a retry may fire without any clock advance; tests advance generously (`drain`) and assert the *outcome* + call count, not a strict pre-advance gate.
> - If clippy `missing_const_for_fn` rejects `const fn new` for generic `T`, drop `const`.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): Retry layer — eligibility, transient+5xx retry, jitter backoff"
```

---

## Task 4: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Append to ADR-0034's **Amendments (2026-07-04)** numbered list (after item 7, the `AuthSource` note #80 added) a new item 8:

```markdown
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
```

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added` (after the Timeout resilience-layer entry #78):

```markdown
- `oath-adapter-net-http-api` `Retry` resilience layer (Slice 1 PR 3) — the
  `Retry<S, T>` service + `RetryLayer<T>` factory (`net-api::Layer`): re-issues an
  explicitly-eligible request (a `Retryable` marker extension; absent → never retried,
  fail-safe) on a transient failure (`HttpError::{Timeout, Connection}`) or a `5xx`
  response, with capped-exponential full-jitter backoff up to `max_attempts`. Never
  retries a 429 / other 4xx / `Auth` / `Throttled`; returns the last outcome verbatim on
  exhaustion; body-transparent (drops a superseded response, releasing its `Guarded`
  permit). Adds the `Retryable` marker + `RetryConfig` schedule; jitter via an internal
  seeded `SplitMix64` — no new dependency. (ADR-0031 §2, ADR-0034.)
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete — no new dep, so `deny`/`machete` are unaffected).

- [ ] **Step 4: Commit, push, PR**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md
git commit -m "docs(net): record Retry layer amendment (ADR-0034 #8) + changelog"
git push -u origin feat/net-http-retry
gh pr create \
  --title "feat(net): Retry resilience layer (Slice 1, PR 3)" \
  --body "Closes #<N>

Slice 1 **PR 3** of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-retry-layer-design.md; ADR-0031 §2). Builds on RateLimit (#76) and Timeout (#78).

- **\`Retry<S, T>\`** + **\`RetryLayer<T>\`** (\`net-api::Layer\`) — order-safe retry: re-issues an **explicitly-eligible** request (a \`Retryable\` marker; **absent → never retried**, so a forgotten stamp never duplicates a \`POST\`) on a **transient** failure (\`HttpError::{Timeout, Connection}\`) or a **5xx** response, with capped-exponential **full-jitter** backoff up to \`max_attempts\`.
- **Never retries a 429 / other 4xx / \`Auth\` / \`Throttled\`** (ADR-0031 §2/§5); returns the **last** outcome verbatim on exhaustion.
- **Body-transparent** — drops a superseded response, releasing its \`Guarded\` permit; \`Auth\`/\`RateLimit\` re-run per attempt (they sit inside \`Retry\`).
- Explicit-only eligibility **tightens ADR-0031 §2** (idempotent-method default → fail-safe opt-in), recorded as **ADR-0034 Amendment #8**.

Runtime-neutral: generic over \`net-api::Timer\`, jitter via an internal seeded \`SplitMix64\` — **no \`rand\`, no new dependency**, no injected \`Jitter\` generic. MockTimer-driven tests with inline service doubles (+ the production \`Guarded\` for the permit-release test).

Next: **Slice 1 PR 4** — the \`CircuitBreaker\` layer.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `Retry<S, T>` + `RetryLayer<T>` (`Layer`), infallible `const fn new` — Task 3. ✅
- `RetryConfig` (`max_attempts: NonZeroU32`, `base`, `cap`, `seed`) — Task 1. ✅
- `Retryable` marker, explicit-only fail-safe (absent → not retried) — Task 1 (type + round-trip) + Task 3 (`eligible` gate + `not_eligible_sends_once…` test). ✅
- Retry decision: transient `{Timeout, Connection}` + 5xx; never 429/4xx/`Auth`/`Throttled`/`Other` — Task 3 (`is_transient` + `is_server_error()` + the 429/4xx/throttled/auth tests). ✅
- Whole-request clone per attempt; `Auth` re-stamps for free — Task 3 (`req.clone()` + doc). ✅
- Drop prior response before backoff (releases `Guarded` permit) — Task 3 (`drop(outcome)` + `prior_response_permit_is_released_before_the_retry` test). ✅
- Last outcome verbatim on exhaustion — Task 3 (`attempts_exhausted_returns_the_last_outcome_verbatim`). ✅
- Capped-exponential full-jitter backoff, overflow-safe — Task 2 (`SplitMix64`/`duration_in`) + Task 3 (`backoff_ceiling`). ✅
- Internal seeded `SplitMix64`, no `rand`, no `Jitter` generic — Task 2. ✅
- Body-transparent `Response<B>`, no `B: Body` bound, `S: Sync` — Task 3. ✅
- No new `HttpError` variant (reuses `ErrorKind` + `StatusCode::is_server_error`) — Task 3. ✅
- MockTimer-driven tests, inline doubles, no `MockClient` — Tasks 1–3. ✅
- ADR-0034 Amendment #8 + CHANGELOG — Task 4. ✅
- Deferred (correctly absent): total-elapsed budget, `Retry-After`, `CircuitBreaker`/`Tracing`, streaming recovery, per-request backoff override, `stack()`/`build()` — noted, not built. ✅

**Placeholder scan:** none — every step carries actual code or an actual command with expected output (`#<N>` is the PR-time issue number, per house convention).

**Type consistency:**
- `Retryable` (ZST) — identical in Task 1's def, Task 3's `get::<Retryable>().is_some()`, and the `req(eligible)` helper.
- `RetryConfig { max_attempts: NonZeroU32, base, cap, seed }` — identical in Task 1's def, Task 3's field reads (`self.cfg.max_attempts.get()`, `self.cfg.base/cap/seed`), and the `cfg(...)` test helper.
- `SplitMix64::{new, duration_in, clone}` — defined Task 2, used Task 3 (`SplitMix64::new(self.cfg.seed)`, `self.rng.duration_in(ceil)`, `self.rng.clone()`).
- `RetryLayer::new(RetryConfig, T) -> Self` and `.layer(inner) -> Retry<S, T>` — match the `Interfaces` block and every test call.
- `Retry` `Service` impl: inner `Response = http::Response<B>` → `Response = http::Response<B>` (transparent) — matches `ScriptLeaf` (`B = StubBody`) and `PermitLeaf` (`B = Guarded<StubBody>`).
- `backoff_ceiling(base, cap, attempt)` / `is_transient(kind)` — defined and called only in Task 3.
- `lib.rs` re-export accumulates to `pub use retry::{Retry, RetryConfig, RetryLayer, Retryable};`; `SplitMix64` stays crate-private (not re-exported).

**Known risks to watch during impl:** listed inline in Task 2 Step 4 (`from_nanos`/`try_from` totality) and Task 3 Step 4 (`Request: Clone`, `S: Sync`, no `B` bound, jitter-zero timing, `const fn` fallback).
