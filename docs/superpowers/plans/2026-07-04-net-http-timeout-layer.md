# net-http `Timeout` Layer (Slice 1, PR 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `Timeout<S, T>` HTTP middleware layer that bounds how long the inner stack may take to **produce a response** — the *send*, not the pacing-permit wait — returning `HttpError::Timeout` when a per-layer (or per-request-overridden) deadline elapses first.

**Architecture:** A `Timer`-generic, runtime-neutral `Service` wrapper in `oath-adapter-net-http-api`. It reads an optional per-request `RequestTimeout(Duration)` extension (absent → the layer default), then races `inner.call(req)` against `Timer::sleep(dur)` via `futures_util::future::select`: the inner future winning yields its `Result` verbatim, the deadline winning yields `HttpError::Timeout` (inner future dropped). **Body-transparent** — `http::Response<B>` is returned untouched (no `Guarded`-style carrier, no `B` bound).

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `futures-util` (the race — already a crate dep since #76), `http`/`bytes`, `std::time::Duration`, `net-api::Timer`. Tests use inline service doubles + `MockTimer` (`oath-adapter-net-mock`), driven on `tokio` (dev-only).

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result`. Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations` — hand-impl where a derive would demand `Debug`/`Clone` on `S`/`T`), `const fn` where `missing_const_for_fn` asks.
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde` in non-dev deps. **This PR adds no dependency** (`futures-util`, `http`, `bytes` are crate deps; `oath-adapter-net-mock` + `tokio` are dev-deps — all present since #76), so `cargo-deny`/`machete` are unaffected.
- **net-http-api tests must NOT dev-depend on `oath-adapter-net-http-mock` (`MockClient`)** — it normal-depends on this crate, so the dev-dep closes a cycle that recompiles a second, non-unifying copy of `net-http-api` (E0599: `MockClient` does not satisfy *this* crate's `Service`). Use **inline** service doubles + `oath-adapter-net-mock`'s `MockTimer`, exactly as `rate_limit.rs`/`body.rs` do.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-04-net-http-timeout-layer-design.md](../specs/2026-07-04-net-http-timeout-layer-design.md), governed by [ADR-0031 §1](../../adr/0031-http-resilience-venue-pacing.md) and [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 1, PR 2** — the second of the resilience-layer PRs (RateLimit #76 landed PR 1; then Timeout, Retry, CircuitBreaker, Tracing).

## File Structure

- `crates/adapter/net/http/api/src/timeout.rs` — **new** (Tasks 1–2). `RequestTimeout`, `TimeoutLayer<T>`, `Timeout<S, T>`, the `Layer`/`Service` impls, and their tests.
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Tasks 1–2). `pub mod timeout;` + re-exports + module-doc bullet.
- `docs/adr/0034-...md`, `CHANGELOG.md` — **modify** (Task 3).

No `Cargo.toml` change. Each task is one or more commits; the tasks together are one PR/issue.

---

## Setup: issue (worktree already exists)

> The isolated worktree **already exists** at `.claude/worktrees/net-http-timeout` (branch `feat/net-http-timeout`, branched off `origin/main` = #76). All tasks run inside it. Only the GitHub issue remains to be created.

- [ ] **Create the issue**

```bash
gh issue create \
  --title "feat(net): Timeout resilience layer (Slice 1, PR 2)" \
  --label enhancement \
  --body "Slice 1 PR 2 of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-timeout-layer-design.md; ADR-0031 §1).

- \`Timeout<S, T>\` + \`TimeoutLayer<T>\` (impl \`net-api::Layer\`): bounds the send (inner call -> response), not the pacing-permit wait — a response-future race against \`Timer::sleep\`, \`HttpError::Timeout\` on the deadline
- \`RequestTimeout(Duration)\` per-request override extension; absent -> layer default (not fail-closed — the global deadline still applies)
- Body-transparent (\`Response<B>\` untouched); a streaming-body \`TimeoutBody\` is deferred (inert on IBKR's buffered traffic). No new dependency."
```

Note the issue number `#<N>` for the PR body.

---

## Task 1: `RequestTimeout` directive + module scaffold

**Files:**
- Create: `crates/adapter/net/http/api/src/timeout.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: nothing (only `std::time::Duration`).
- Produces:
  - `oath_adapter_net_http_api::RequestTimeout` — `struct RequestTimeout(pub Duration)` (`Debug`, `Clone`, `Copy`), an `http::Request` extension.
  - Task 2 adds `TimeoutLayer`/`Timeout` to this module and the `Service` race.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/timeout.rs` with the module doc + only the directive test below:

```rust
//! The `Timeout` resilience layer (ADR-0031 §1).
//!
//! Bounds how long the inner stack may take to **produce a response** — the
//! *send*, not the pacing-permit wait (`RateLimit` sits outside it, so a
//! throttled request never enters `Timeout`). Races `inner.call(req)` against
//! [`Timer::sleep`](oath_adapter_net_api::Timer::sleep); the deadline winning
//! yields [`HttpError`]`::Timeout` with the inner future dropped, the inner
//! finishing first yields its `Result` verbatim. **Body-transparent:** the
//! response body is returned untouched. The per-request [`RequestTimeout`]
//! extension overrides the layer default; an absent extension uses the default.
//! Runtime-neutral: generic over [`Timer`](oath_adapter_net_api::Timer), race
//! via `futures-util`.

use std::time::Duration;

/// A per-request timeout override, carried as an `http::Request` extension.
///
/// The adapter stamps it for an endpoint that needs a non-default bound. `Copy`
/// so it survives the per-attempt request clone `Retry` performs (Slice 1). An
/// **absent** extension uses the layer default — a missing override has no
/// fail-open hazard (the global deadline still applies), so it is not rejected
/// (contrast `RateScope`, ADR-0034 Amendment #1).
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout(pub Duration);

#[cfg(test)]
mod tests {
    use super::RequestTimeout;
    use std::time::Duration;

    #[test]
    fn request_timeout_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut().insert(RequestTimeout(Duration::from_secs(3)));
        let got = req
            .extensions()
            .get::<RequestTimeout>()
            .copied()
            .expect("override present");
        assert_eq!(got.0, Duration::from_secs(3));
    }
}
```

In `lib.rs`, add the module-doc bullet (after the `rate_limit` bullet, line 14), the `pub mod`, and the re-export (keep alphabetical ordering — `timeout` sits after `service`):

Module-doc bullet (insert after line 14):

```rust
//! - [`timeout`] — the `Timeout` layer, its `TimeoutLayer` factory, and the
//!   `RequestTimeout` per-request override
```

Module declaration (after `pub mod service;`):

```rust
pub mod timeout;
```

Re-export (after `pub use service::Service;`):

```rust
pub use timeout::RequestTimeout;
```

(Task 2 extends this to `pub use timeout::{RequestTimeout, Timeout, TimeoutLayer};`.)

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — before adding the code, `cannot find type RequestTimeout in module timeout` / unresolved module `timeout`. After adding the code it should compile; the step exists to confirm the wiring, so if it already passes, proceed.

- [ ] **Step 3: Confirm the test passes**

Run: `cargo test -p oath-adapter-net-http-api timeout && just lint`
Expected: PASS, warning-free. (`RequestTimeout` is fully implemented in Step 1; this task has no separate implementation step.)

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/timeout.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): RequestTimeout per-request timeout override extension"
```

---

## Task 2: `Timeout` layer — the response-future race

**Files:**
- Modify: `crates/adapter/net/http/api/src/timeout.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `RequestTimeout` (Task 1); `HttpError`, `Service` (crate); `Layer`, `Timer` (`oath_adapter_net_api`); `futures_util::future::{Either, select}`.
- Produces:
  - `oath_adapter_net_http_api::TimeoutLayer<T>` — `impl Layer<S>` factory; `pub const fn new(default: Duration, timer: T) -> Self`.
  - `oath_adapter_net_http_api::Timeout<S, T>` — the wrapping `Service`; for an inner `S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync` and `T: Timer`, it is `Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>` (body-transparent — same `B`).

- [ ] **Step 1: Write the failing tests**

Add the imports + inline doubles + tests to the `tests` module in `timeout.rs` (replace the `use super::RequestTimeout;` line):

```rust
    use super::{RequestTimeout, Timeout, TimeoutLayer};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::{Layer, Timer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    // A canned one-frame response body (`Data = Bytes`, `Error = HttpError`) —
    // enough to prove `Timeout` returns the body untouched. `Debug` so
    // `Result::unwrap_err` can render an unexpected `Ok`.
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

    // An inline leaf returning `200` immediately — the fast path. Inline (not
    // `MockClient`) to avoid the net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct FastLeaf {
        body: &'static [u8],
    }
    impl Service<http::Request<Bytes>> for FastLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let body = self.body;
            async move { Ok(http::Response::new(StubBody::new(body))) }
        }
    }

    // An inline leaf that sleeps `delay` on the shared clock before returning —
    // lets a test hold the inner future pending while the layer deadline fires,
    // or (with a finite delay) complete after the deadline would have.
    #[derive(Clone)]
    struct DelayLeaf<T> {
        timer: T,
        delay: Duration,
        completed: Arc<AtomicBool>,
    }
    impl<T: Timer> DelayLeaf<T> {
        fn new(timer: T, delay: Duration) -> Self {
            Self { timer, delay, completed: Arc::new(AtomicBool::new(false)) }
        }
        fn completed(&self) -> bool {
            self.completed.load(Ordering::Relaxed)
        }
    }
    impl<T: Timer> Service<http::Request<Bytes>> for DelayLeaf<T> {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let timer = self.timer.clone();
            let delay = self.delay;
            let completed = self.completed.clone();
            async move {
                timer.sleep(delay).await;
                completed.store(true, Ordering::Relaxed);
                Ok(http::Response::new(StubBody::new(b"slow")))
            }
        }
    }

    // An inline leaf returning a `Connection` error immediately.
    #[derive(Clone)]
    struct ErrLeaf;
    impl Service<http::Request<Bytes>> for ErrLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async move { Err(HttpError::connection("reset")) }
        }
    }

    fn req(override_to: Option<Duration>) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        if let Some(d) = override_to {
            r.extensions_mut().insert(RequestTimeout(d));
        }
        r
    }

    #[tokio::test]
    async fn fast_inner_passes_and_body_is_transparent() {
        let svc = TimeoutLayer::new(Duration::from_secs(1), MockTimer::new()).layer(FastLeaf { body: b"ok" });
        let resp = svc.call(req(None)).await.expect("inner within deadline");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<B> passed straight through
    }

    #[tokio::test]
    async fn slow_inner_times_out_at_default() {
        let timer = MockTimer::new();
        let leaf = DelayLeaf::new(timer.clone(), Duration::from_secs(3600));
        let leaf_probe = leaf.clone();
        let svc = TimeoutLayer::new(Duration::from_secs(1), timer.clone()).layer(leaf);
        let waiter = tokio::spawn(async move { svc.call(req(None)).await });
        tokio::task::yield_now().await; // task registers inner sleep(3600s) + deadline sleep(1s)
        timer.advance(Duration::from_secs(1)); // fire the layer deadline
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Timeout)); // HttpError has no PartialEq
        assert!(!leaf_probe.completed(), "inner future must be dropped, not run to completion");
    }

    #[tokio::test]
    async fn request_timeout_override_shortens_deadline() {
        // Layer default is huge; a per-request 1s override fires first.
        let timer = MockTimer::new();
        let svc = TimeoutLayer::new(Duration::from_secs(3600), timer.clone())
            .layer(DelayLeaf::new(timer.clone(), Duration::from_secs(3600)));
        let waiter = tokio::spawn(async move { svc.call(req(Some(Duration::from_secs(1)))).await });
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // fires the override, not the default
        let err = waiter.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Timeout));
    }

    #[tokio::test]
    async fn request_timeout_override_lengthens_deadline() {
        // Default 1s would time out; a 5s override lets the 2s inner complete.
        let timer = MockTimer::new();
        let svc = TimeoutLayer::new(Duration::from_secs(1), timer.clone())
            .layer(DelayLeaf::new(timer.clone(), Duration::from_secs(2)));
        let waiter = tokio::spawn(async move { svc.call(req(Some(Duration::from_secs(5)))).await });
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // now=1s: neither the 2s inner nor the 5s override is due
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(1)); // now=2s: the inner completes first
        let resp = waiter.await.unwrap().expect("override outlived the default; inner completed");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"slow"));
    }

    #[tokio::test]
    async fn inner_error_passes_through_not_masked_as_timeout() {
        let svc = TimeoutLayer::new(Duration::from_secs(1), MockTimer::new()).layer(ErrLeaf);
        let err = svc.call(req(None)).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_))); // its own error, never Timeout
    }

    #[tokio::test]
    async fn zero_default_still_returns_ready_inner() {
        // `select` polls the inner call first, so a ready inner is never
        // preempted by a Duration::ZERO deadline.
        let svc = TimeoutLayer::new(Duration::ZERO, MockTimer::new()).layer(FastLeaf { body: b"ok" });
        svc.call(req(None)).await.expect("inner polled first, not the zero deadline");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Timeout`/`TimeoutLayer` in module `timeout`.

- [ ] **Step 3: Implement the layer**

Insert the imports + types between the `RequestTimeout` definition and the `tests` module in `timeout.rs`. Extend the top-of-file `use` block:

```rust
use crate::{HttpError, Service};
use bytes::Bytes;
use futures_util::future::{Either, select};
use oath_adapter_net_api::{Layer, Timer};
use std::fmt;
use std::future::Future;
use std::time::Duration;
```

(The existing `use std::time::Duration;` from Task 1 is now covered by this block — keep a single `Duration` import.)

Add below `RequestTimeout`:

```rust
/// The `Timeout` [`Layer`] factory: holds the default deadline + clock and
/// produces a [`Timeout`] around any inner service.
pub struct TimeoutLayer<T> {
    default: Duration,
    timer: T,
}

impl<T> TimeoutLayer<T> {
    /// Build the layer with a default deadline and a [`Timer`] clock.
    ///
    /// The default bounds every request lacking a [`RequestTimeout`] extension.
    /// Infallible — every [`Duration`] is a valid deadline (no config to check).
    #[must_use]
    pub const fn new(default: Duration, timer: T) -> Self {
        Self { default, timer }
    }
}

impl<T: Clone> Clone for TimeoutLayer<T> {
    fn clone(&self) -> Self {
        Self { default: self.default, timer: self.timer.clone() }
    }
}

impl<T> fmt::Debug for TimeoutLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeoutLayer").field("default", &self.default).finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for TimeoutLayer<T> {
    type Service = Timeout<S, T>;

    fn layer(&self, inner: S) -> Timeout<S, T> {
        Timeout { inner, default: self.default, timer: self.timer.clone() }
    }
}

/// The `Timeout` middleware: races the inner call against a deadline, returning
/// [`HttpError`]`::Timeout` if the deadline wins. Body-transparent — the inner
/// `http::Response<B>` is returned untouched.
pub struct Timeout<S, T> {
    inner: S,
    default: Duration,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for Timeout<S, T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), default: self.default, timer: self.timer.clone() }
    }
}

impl<S, T> fmt::Debug for Timeout<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timeout").field("default", &self.default).finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for Timeout<S, T>
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
            let dur = req
                .extensions()
                .get::<RequestTimeout>()
                .map_or(self.default, |t| t.0);
            // `select` polls `call` first, so a ready inner beats a zero deadline;
            // pinning to the stack makes both futures `Unpin` for `select`.
            let call = std::pin::pin!(self.inner.call(req));
            let nap = std::pin::pin!(self.timer.sleep(dur));
            match select(call, nap).await {
                Either::Left((res, _)) => res, // inner finished first -> its Result verbatim
                Either::Right(((), _)) => Err(HttpError::Timeout), // deadline won -> inner dropped
            }
        }
    }
}
```

In `lib.rs`, extend the Task 1 re-export:

```rust
pub use timeout::{RequestTimeout, Timeout, TimeoutLayer};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api timeout && just lint`
Expected: PASS, warning-free.

> Known risks (from the spec's implementation notes):
> - `select` needs `Unpin` futures — `std::pin::pin!` both (shown). It polls the left (`call`) first, so an immediately-ready inner is never preempted by a `Duration::ZERO` deadline.
> - `S: Sync` is required because the returned `Send` future borrows `&self` (`&S: Send` ⇒ `S: Sync`; `T: Sync` holds via `Timer`).
> - `B` carries **no** `http_body::Body` bound — `Timeout` never touches the body, so it stays fully generic (contrast `RateLimit`, which builds `Guarded<B>`).
> - If clippy's `missing_const_for_fn` rejects `const fn new` for the generic `T` (it should not — construction-only), drop `const`.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/timeout.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): Timeout layer — response-future race, body-transparent"
```

---

## Task 3: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Append to ADR-0034's **Amendments (2026-07-04)** numbered list (after item 5, the RateLimit note #76 added) a new item 6:

```markdown
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
```

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added` (after the RateLimit resilience-layer entry #76):

```markdown
- `oath-adapter-net-http-api` `Timeout` resilience layer (Slice 1 PR 2) — the
  `Timeout<S, T>` service + `TimeoutLayer<T>` factory (`net-api::Layer`): bounds the
  send (inner call → response) against a `net-api::Timer` deadline, returning
  `HttpError::Timeout` when it elapses first (inner future dropped); body-transparent.
  Adds the `RequestTimeout(Duration)` per-request override extension (absent → the
  layer default). Response-future-only (ADR-0031 §1's "bounds the send, not the permit
  wait"); a streaming-body timeout is deferred. No new dependency. (ADR-0031 §1,
  ADR-0034.)
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete — no new dep, so `deny`/`machete` are unaffected).

- [ ] **Step 4: Commit, push, PR**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md
git commit -m "docs(net): record Timeout layer amendment (ADR-0034) + changelog"
git push -u origin feat/net-http-timeout
gh pr create \
  --title "feat(net): Timeout resilience layer (Slice 1, PR 2)" \
  --body "Closes #<N>

Slice 1 **PR 2** of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-timeout-layer-design.md; ADR-0031 §1). Builds on the RateLimit layer (#76).

- **\`Timeout<S, T>\`** + **\`TimeoutLayer<T>\`** (\`net-api::Layer\`) — bounds the **send** (inner call → response), not the pacing-permit wait (ADR-0031 §1): a response-future race against \`Timer::sleep\`, returning \`HttpError::Timeout\` when the deadline wins (inner future dropped). Body-transparent — \`http::Response<B>\` returned untouched.
- **\`RequestTimeout(Duration)\`** per-request override extension — absent → the layer default (not fail-closed: a missing override has no fail-open pacing hazard, unlike \`RateScope\`).
- A streaming-body \`TimeoutBody\` is **deferred** — inert on IBKR's all-buffered responses; a clean additive follow-up when a streaming venue lands.

Runtime-neutral: generic over \`net-api::Timer\`, race via \`futures-util\` — no \`tokio\`/\`hyper\`. **No new dependency.** MockTimer-driven tests with inline service doubles.

Next: **Slice 1 PR 3** — the \`Retry\` layer.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `Timeout<S, T>` + `TimeoutLayer<T>` (`Layer`), response-future race → Task 2. ✅
- `RequestTimeout(Duration)` extension; absent → default (not fail-closed) — Task 1 (type) + Task 2 (`map_or(self.default, …)` + tests). ✅
- Body-transparent `Response<B>`, no `B: Body` bound — Task 2. ✅
- Infallible `const fn new`, hand-written `Clone`/`Debug`, `S: Sync` bound — Task 2. ✅
- `HttpError::Timeout` reuse (no new variant) — Task 2 (`Either::Right`). ✅
- `select` polls inner first (zero-deadline ordering) — Task 2 test `zero_default_still_returns_ready_inner`. ✅
- Inner error passes through unchanged — Task 2 test `inner_error_passes_through_not_masked_as_timeout`. ✅
- MockTimer-driven tests, inline doubles, no `MockClient` — Tasks 1–2. ✅
- ADR-0034 Amendment #6 + CHANGELOG — Task 3. ✅
- Deferred (correctly absent): `TimeoutBody`, `Retry`/`CircuitBreaker`/`Tracing`, `stack()`/`build()`, tokio `Timer` — noted, not built. ✅

**Placeholder scan:** none — every step carries actual code or an actual command with expected output.

**Type consistency:**
- `RequestTimeout(pub Duration)` — identical in Task 1's def, Task 2's `map_or(self.default, |t| t.0)`, and the `req()` helper.
- `TimeoutLayer::new(Duration, T) -> Self` and `.layer(inner) -> Timeout<S, T>` — match the `Interfaces` block and every test call.
- `Timeout` `Service` impl: inner `Response = http::Response<B>` → `Response = http::Response<B>` (transparent) — matches `FastLeaf`/`DelayLeaf`/`ErrLeaf` (`Response = http::Response<StubBody>`, so `B = StubBody`).
- `select` arms `Either::Left((res, _))` / `Either::Right(((), _))` — consistent with `futures_util::future::select` over `call: Result<…>` and `nap: ()`.
- `lib.rs` re-export accumulates to `pub use timeout::{RequestTimeout, Timeout, TimeoutLayer};`.

**Known risks to watch during impl:** listed inline in Task 2 Step 4 (`select` `Unpin`/poll-order, `S: Sync`, no `B` bound, `const fn` fallback).
