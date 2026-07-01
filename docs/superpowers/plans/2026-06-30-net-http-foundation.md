# net-http Foundation (Slice 0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lay the foundation the net-http construction surface sits on — execute the ADR-0029 transport-split repartition (move `Service` out of the kernel), add the kernel `Timer` contract, and birth `oath-adapter-net-http-api` — so the contract work in PRs 2–4 has a place to land.

**Architecture:** The kernel (`oath-adapter-net-api`) keeps the transport-neutral composition machinery (`Layer`, `ServiceBuilder`, `Stack`, `Identity`), classification (`ErrorKind`, `HasErrorKind`), and gains the runtime-neutral `Timer` clock — and becomes **std-only** (the signal the cut is clean, ADR-0029 §3). `Service` moves up into the new per-transport contract crate `oath-adapter-net-http-api`, which depends inward on the kernel. No `dyn`, RPITIT throughout (ADR-0007/0029 §5).

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just` task runner, the workspace `[workspace.lints]` gate. PR 1 introduces no new external dependencies.

**Source spec:** [docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md](../specs/2026-06-30-net-http-construction-surface-design.md). This plan is **Slice 0 (Foundation)** of that spec; Slices 1–2 (layers, assembly+backend) are later plans.

## Global Constraints

Every task implicitly includes these (verbatim from the workspace manifest, CLAUDE.md, and the spec):

- **Edition 2024, MSRV 1.90** (`rust-version = "1.90"`; toolchain pinned `1.96.0`). Validate MSRV with `just msrv`.
- **No `unsafe`** — `unsafe_code = "deny"` workspace-wide. Body impls (later PRs) use `pin-project-lite`, never manual `unsafe`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — `unwrap_used`/`expect_used`/`indexing_slicing`/`panic_in_result_fn` are warn-level but **CI is warning-free**, so treat as deny. Test code is exempt via `.clippy.toml`.
- **clippy `all` = deny**; `pedantic`/`nursery`/`cargo` = warn (must stay clean); `dbg_macro` = deny.
- **Document all public items** (`missing_docs` = warn); **derive `Debug`** everywhere (`missing_debug_implementations` = warn); **no `pub` that isn't reachable** (`unreachable_pub` = warn).
- **Definition of done per PR:** `just ci` green (`fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck`). The pre-push hook enforces this.
- **Dependencies:** internal crates via `[workspace.dependencies]` with an explicit `version`; external shared deps also via `[workspace.dependencies]`.
- **`oath-adapter-net-http-api` charter:** no `tokio`/`hyper`/`reqwest`/`serde`; free of any async runtime (ADR-0030, as amended by the spec). In PR 1 it has **zero** dependencies.
- **Workflow:** one issue → one branch off `main` → isolated git worktree under `.claude/worktrees/<slug>` (never switch the primary checkout) → one PR that `Closes #N`. Update `CHANGELOG.md` `[Unreleased]` in each PR.

---

## File Structure

PR 1 touches:

- `crates/adapter/net/api/src/timer.rs` — **new.** The `Timer` clock contract (kernel).
- `crates/adapter/net/api/src/compose.rs` — **renamed** from `service.rs`; holds `Layer`, `ServiceBuilder`, `Stack`, `Identity` (the `Service` trait removed).
- `crates/adapter/net/api/src/lib.rs` — re-exports updated (`Timer` added, `Service` dropped, module renamed).
- `crates/adapter/net/api/Cargo.toml` — stripped to **std-only** (external deps + machete block removed).
- `crates/adapter/net/http/api/Cargo.toml` — **new crate** `oath-adapter-net-http-api`.
- `crates/adapter/net/http/api/src/lib.rs` — **new.** Crate root.
- `crates/adapter/net/http/api/src/service.rs` — **new.** The `Service` trait (moved from the kernel).
- `Cargo.toml` (workspace) — add the new member + its `[workspace.dependencies]` entry.
- `README.md` — crate table + mermaid dependency graph.
- `CHANGELOG.md` — `[Unreleased]`.

## Slice 0 PR map

| PR | Scope | Status |
|----|-------|--------|
| **PR 1** | ADR-0029 repartition + kernel `Timer` + birth of `net-http-api` | **this plan, full detail** |
| PR 2 | HTTP transport contract (`HttpError`, `HttpClient`, `ResponseBody`+forwarding, `BufferMode`) + the `net-http-mock` harness (`MockClient`, `MockTimer`, `MockBody`) | roadmap below → own plan |
| PR 3 | `AuthSource` + `Auth` layer + `NoAuth`; `Guarded<B>` body (forwarding + eager release) | roadmap below → own plan |
| PR 4 | `RateKey` + `RateLimitConfig<K>` + `LimitDecl` + `BuildError` + boot-time coverage validation | roadmap below → own plan |

---

## PR 1: ADR-0029 repartition + kernel `Timer` + `net-http-api` birth

**Issue:** "feat(net): ADR-0029 repartition — move `Service` to `net-http-api`, add kernel `Timer`". **Branch:** `feat/net-http-api-repartition`.

### Task 1.1: Kernel — add the `Timer` contract

**Files:**
- Create: `crates/adapter/net/api/src/timer.rs`
- Modify: `crates/adapter/net/api/src/lib.rs`

**Interfaces:**
- Produces: `oath_adapter_net_api::Timer` — `trait Timer: Clone + Send + Sync { fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send; fn now(&self) -> Instant; }`

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/api/src/timer.rs` with only the test, and add `pub mod timer;` to `lib.rs` (temporarily, before the trait exists):

```rust
//! The `Timer` clock contract — placeholder; trait added in step 3.

#[cfg(test)]
mod tests {
    use super::Timer;
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct FixedTimer(Instant);

    impl Timer for FixedTimer {
        fn sleep(&self, _dur: Duration) -> impl std::future::Future<Output = ()> + Send {
            std::future::ready(())
        }
        fn now(&self) -> Instant {
            self.0
        }
    }

    #[test]
    fn now_returns_the_configured_instant() {
        let t0 = Instant::now();
        assert_eq!(FixedTimer(t0).now(), t0);
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `just check`
Expected: FAIL — `cannot find trait Timer in this module`.

- [ ] **Step 3: Add the `Timer` trait**

Prepend to `timer.rs` (above the `#[cfg(test)]` block), and replace the placeholder `//!` line with real module docs:

```rust
//! The `Timer` clock contract — a runtime-neutral clock for timing layers.

use std::future::Future;
use std::time::{Duration, Instant};

/// A clock abstraction for timing layers, decoupled from any async runtime.
///
/// Timing middleware (`Timeout`, `Retry` backoff, `RateLimit` refill,
/// `CircuitBreaker` cooldown) is generic over `Timer` so a mock clock can drive
/// it deterministically in tests while production passes a runtime-backed impl.
/// A trait — not a runtime — so the kernel stays std-only (ADR-0029 §4).
pub trait Timer: Clone + Send + Sync {
    /// Complete after `dur` has elapsed.
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send;

    /// The current instant — for elapsed-time reads (token-bucket refill,
    /// circuit cooldown).
    fn now(&self) -> Instant;
}
```

- [ ] **Step 4: Re-export and run the test**

In `lib.rs` add `pub use timer::Timer;` next to the other re-exports, and add `//! - [`timer`] — `Timer`` to the module-list doc comment.

Run: `just check && cargo test -p oath-adapter-net-api timer`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/api/src/timer.rs crates/adapter/net/api/src/lib.rs
git commit -m "feat(net): add Timer clock contract to the kernel"
```

> **Runtime-neutrality demonstration (optional, recommended).** Add one dev test that polls `FixedTimer::sleep(..)` via a minimal std `block_on` (a noop-waker loop — zero new deps) instead of `#[tokio::test]`, proving the kernel's futures run off *any* runtime. The token gesture is weak here (`sleep` is a ready future); the meaningful demonstration — a layer driven over a mock executor — lands with the layer slices.

### Task 1.2: Create `oath-adapter-net-http-api` and move `Service` into it

**Files:**
- Modify: `Cargo.toml` (workspace `members` + `[workspace.dependencies]`)
- Create: `crates/adapter/net/http/api/Cargo.toml`
- Create: `crates/adapter/net/http/api/src/service.rs`
- Create: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Produces: `oath_adapter_net_http_api::Service` — `trait Service<Req> { type Response; type Error; fn call(&self, req: Req) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send; }` (moved verbatim from the kernel).

- [ ] **Step 1: Register the crate in the workspace**

In the root `Cargo.toml`, add to `members` (after `"crates/adapter/net/api"`):

```toml
  "crates/adapter/net/http/api",
```

and to `[workspace.dependencies]` (after the `oath-adapter-net-api` line):

```toml
oath-adapter-net-http-api = { path = "crates/adapter/net/http/api", version = "0.1.0" }
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/adapter/net/http/api/Cargo.toml`. **No `[dependencies]`** — `Service<Req>` is generic and uses only `std::future::Future` (HTTP/kernel deps arrive in PR 2):

```toml
[package]
name = "oath-adapter-net-http-api"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Move the `Service` trait in**

Create `crates/adapter/net/http/api/src/service.rs` with the trait lifted verbatim from the kernel's `service.rs` (its doc-comment included):

```rust
//! The request/reply connection-shape contract for the HTTP transport.
//!
//! `Service` models **request → one reply** — it fits REST and unary RPC but
//! not WebSocket subscriptions or multicast, so it is a per-transport contract,
//! not a kernel primitive (ADR-0029 §2). It is transport-*neutral* (names no
//! HTTP type); it lives here, the first request/reply transport, until a second
//! one justifies hoisting it into a shared `net-req-reply-api` crate.

use std::future::Future;

/// A single async call: request in, `Result` out.
///
/// `call` takes `&self` (not `&mut self`) — a service is shared, not owned, by
/// its callers. The returned future is **`Send`** (enforced here) so it runs on
/// a multithreaded runtime. The service *value* is expected to be
/// `Send + Sync + 'static` too, but that is enforced at the **composition
/// boundary** (`stack()`'s return bound, ADR-0030), not on this trait — so a
/// service may be non-`Sync` in a context that never shares it. Backpressure is
/// handled inside `call` (e.g. awaiting a permit), not via a separate
/// `poll_ready`.
///
/// Because the `call` future borrows `&self`, it is **not** `'static`-spawnable:
/// to `tokio::spawn` a call, clone the (cheap, `Arc`-backed) service and move the
/// clone in. RPITIT return — no `async-trait`, no `dyn`, no per-call allocation.
pub trait Service<Req> {
    /// The value produced on success.
    type Response;

    /// The error produced on failure.
    type Error;

    /// Drive the request to completion.
    fn call(&self, req: Req) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send;
}
```

- [ ] **Step 4: Create the crate root**

Create `crates/adapter/net/http/api/src/lib.rs`:

```rust
//! `oath-adapter-net-http-api` — the HTTP transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`) and adds the request/reply [`Service`] connection shape. The HTTP
//! data plane (`HttpError`, `HttpClient`, `ResponseBody`, the layers) lands in
//! later slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
#![forbid(unsafe_code)]

pub mod service;

pub use service::Service;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p oath-adapter-net-http-api`
Expected: PASS. (`Service` now exists in both crates transiently — resolved in Task 1.3.)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/adapter/net/http/api
git commit -m "feat(net): create oath-adapter-net-http-api with the Service contract"
```

### Task 1.3: Remove `Service` from the kernel; rename `service` → `compose`; go std-only

**Files:**
- Rename: `crates/adapter/net/api/src/service.rs` → `crates/adapter/net/api/src/compose.rs`
- Modify: `crates/adapter/net/api/src/lib.rs`
- Modify: `crates/adapter/net/api/Cargo.toml`

- [ ] **Step 1: Rename the module file**

```bash
git mv crates/adapter/net/api/src/service.rs crates/adapter/net/api/src/compose.rs
```

- [ ] **Step 2: Delete the `Service` trait and fix the doctest**

In `compose.rs`: remove the `pub trait Service<Req> { … }` block and the now-unused `use std::future::Future;`. Replace the module-doc example (which used `Service`) with a `Layer`-only one, and update the module heading:

```rust
//! Composition machinery: `Layer`, `ServiceBuilder`, `Identity`, `Stack`.
//!
//! These compose **anything** — `Layer<S>` carries no `Service` bound (ADR-0029
//! §3), so the same machinery composes an HTTP `Service` stack today and a WS
//! subscription stack tomorrow.
//!
//! # Ordering invariant
//!
//! The **first** `.layer()` call is permanently the outermost wrapper and
//! therefore the first to handle each request.
//!
//! ```
//! # use oath_adapter_net_api::compose::{Layer, ServiceBuilder};
//! # struct TracingLayer;
//! # struct MetricsLayer;
//! # impl<S> Layer<S> for TracingLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! # impl<S> Layer<S> for MetricsLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! // TracingLayer is added first → outermost → wraps everything else.
//! let _svc = ServiceBuilder::new()
//!     .layer(TracingLayer) // outermost
//!     .layer(MetricsLayer) // inner
//!     .service(());        // leaf: any value (a `Service` leaf lives in net-http-api)
//! ```
```

Also fix the `ServiceBuilder` struct's own doctest import path (`oath_adapter_net_api::service::` → `oath_adapter_net_api::compose::`).

- [ ] **Step 3: Update `lib.rs`**

```rust
//! `oath-adapter-net-api` — transport-neutral composition primitives + contracts.
//!
//! This crate is **std-only** (zero deps — the signal the ADR-0029 cut is
//! clean). It defines the shared abstractions every transport's layers depend
//! on:
//!
//! - [`compose`] — `Layer`, `ServiceBuilder`, `Identity`, `Stack`
//! - [`error_kind`] — `ErrorKind`, `HasErrorKind`
//! - [`timer`] — `Timer`
//!
//! `Service` is **not** here — it is a per-transport contract in
//! `oath-adapter-net-http-api` (ADR-0029 §2).
#![forbid(unsafe_code)]

pub mod compose;
pub mod error_kind;
pub mod timer;

pub use compose::{Identity, Layer, ServiceBuilder, Stack};
pub use error_kind::{ErrorKind, HasErrorKind};
pub use timer::Timer;
```

- [ ] **Step 4: Strip the kernel to std-only**

Replace `crates/adapter/net/api/Cargo.toml` `[dependencies]` and the `[package.metadata.cargo-machete]` block — the HTTP deps belong in `net-http-api`, and the kernel uses none of them:

```toml
[package]
name = "oath-adapter-net-api"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true
```

- [ ] **Step 5: Verify the whole workspace**

Run: `just check && just test && just lint`
Expected: PASS — kernel is std-only with no `Service`; doctests compile; `net-http-api` owns `Service`. (`just machete` no longer has an ignore block to satisfy.)

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/api
git commit -m "refactor(net): kernel keeps composition+ErrorKind+Timer, sheds Service and all deps"
```

### Task 1.4: Update the README dependency graph and CHANGELOG

**Files:**
- Modify: `README.md` (crate table + mermaid graph)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Update the crate table**

In `README.md`, replace the `oath-adapter-net-api` row and add the new crate's row:

```markdown
| `oath-adapter-net-api` | Transport-neutral composition primitives (`Layer`, `ServiceBuilder`, `Stack`) + `ErrorKind` / `Timer` |
| `oath-adapter-net-http-api` | HTTP transport contract (`Service`, …) over the kernel |
```

- [ ] **Step 2: Update the mermaid graph**

In the `graph TD` block, replace the standalone `netapi[oath-adapter-net-api]` line with the kernel node plus the new crate depending on it:

```
    netapi[oath-adapter-net-api]
    nethttpapi[oath-adapter-net-http-api] --> netapi
```

- [ ] **Step 3: Update the CHANGELOG**

In `CHANGELOG.md` under `## [Unreleased]` → `### Changed`, add:

```markdown
- Began the ADR-0029 network-adapter repartition: `oath-adapter-net-api` is now the
  transport-neutral, **std-only** kernel (composition machinery + `ErrorKind` +
  the new runtime-neutral `Timer` clock); the `Service` request/reply contract moved
  into the new per-transport crate `oath-adapter-net-http-api`.
```

- [ ] **Step 4: Full CI gate**

Run: `just ci`
Expected: PASS (all of fmt, fmt-toml, typos, lint, check, test, deny, doc, machete, gitleaks, actionlint, shellcheck).

- [ ] **Step 5: Commit, push, open the PR**

```bash
git add README.md CHANGELOG.md
git commit -m "docs(net): record the ADR-0029 repartition (Service → net-http-api, kernel Timer)"
git push -u origin feat/net-http-api-repartition
gh pr create --fill --base main
```

The PR body must reference `Closes #<issue>`.

---

## Remaining Slice 0 work (PRs 2–4) — roadmap

Each becomes its **own full plan** (authored when its predecessor merges, so the code is written against the types that actually exist). The contract shapes are already fixed in the [design spec](../specs/2026-06-30-net-http-construction-surface-design.md) — these entries capture scope, files, the key interfaces, and the test focus.

### PR 2 — HTTP transport contract + mock harness

- **Crate deps (net-http-api):** add `oath-adapter-net-api`, `http`, `bytes`, `http-body`, `http-body-util`, `pin-project-lite`, `thiserror`, `tracing` to `[workspace.dependencies]`/the crate (as used).
- **`HttpError`** (`src/error.rs`) — one concrete `thiserror` enum for **transport/middleware failures only**: `Timeout`, `Connection(#[source] BoxError)`, `Throttled` (a `RateLimit` max-wait *decision*, not a 429 response), `Auth(String)` + `HttpError::auth(msg)`, `Other(#[source] BoxError)`; `impl HasErrorKind`. (`CircuitOpen` is added with the CircuitBreaker layer — Slice 1.) **HTTP error *statuses* (4xx/5xx) are NOT converted to `HttpError`** — they flow through as `Ok(http::Response)` with the body intact, so the adapter reads the venue's rejection payload and the stack never discards it. This matches ADR-0030 §5 (whose `HttpError` examples are middleware failures — timeout, retry-exhausted, body-read — never statuses); the earlier `Client(StatusCode)`/`Server(StatusCode)` sketch was lossy and is dropped. The resilience layers (`Retry`/`CircuitBreaker`, Slice 1) decide by **peeking `Response::status()`** (5xx → server-error signal) and the 429 **`Retry-After`** header (so `CircuitBreaker` honours the server's cooldown hint instead of a fixed 15 min) — read-only; the `Response` continues downstream unchanged.
- **`HttpClient`** (`src/client.rs`) — the blanket-impl'd `Service` sub-trait with `type Body` and `send` sugar (spec/0030 §6).
- **`ResponseBody<B>`** (`src/body.rs`) — newtype over `Either<MapErr<Full<Bytes>>, B>` with `buffered`/`streaming` constructors; **`Body` impl forwards `poll_frame`, `is_end_stream`, AND `size_hint`** (the spec's transparency fix).
- **`BufferMode`** (`src/body.rs`) — `enum { Buffer, Stream }`, `Copy`.
- **`oath-adapter-net-http-mock`** (`crates/adapter/net/http/mock`, **new crate**, consumed only via `[dev-dependencies]`): `MockClient` (impl `Service<http::Request<Bytes>>` → canned `http::Response<MockBody>`, records requests), `MockBody` (configurable frames + `is_end_stream`/`size_hint`), and `MockTimer`.
  - **`MockTimer` is a controllable clock, not a no-op.** `std::time::Instant` has no value constructor, so it anchors to `Instant::now()` at construction and advances via a stored offset behind interior mutability (`now(&self)` is `&self` but the clock mutates → `Arc<Mutex<…>>` / atomic-nanos). `sleep(dur)` must make elapsed time **observable**: register a wake at `now()+dur`, and a test-only `advance(dur)` bumps the clock and wakes due sleepers (prefer register-wake + `advance` over naive *sleep-advances-now* so concurrent sleepers don't double-count). A no-op `sleep` + fixed `now()` makes Slice 1's token-bucket-refill / circuit-cooldown tests **vacuous**. Prior art: `governor::clock::{Clock, FakeRelativeClock}` and tokio's `pause()`/`advance()`. Keep concrete `std::Instant` (a governor-style `Clock::Instant` associated type was rejected — it makes every timing layer generic over the instant type; concrete keeps them monomorphic, at the cost of this anchor-and-offset recipe).
  - **Dev-dep cycle (legal, flagged):** `net-http-api` dev-depends on `net-http-mock`, which normal-depends on `net-http-api` — Cargo permits this (dev-deps are outside the library build graph). If `just machete` or rust-analyzer trips on the cycle, add a `cargo-machete` ignore; or keep `net-http-api`'s own unit tests on tiny inline doubles and reserve `net-http-mock` for downstream consumers, avoiding the cycle entirely.
- **Tests:** `MockClient` satisfies `HttpClient` (blanket impl applies); `ResponseBody` reports `size_hint`/`is_end_stream` identical to its inner `MockBody` (parity); `MockTimer` makes elapsed time observable — `advance(dur)` after a `sleep(dur)` moves `now()` (driven on a non-tokio executor where practical, to demonstrate runtime-neutrality); `HttpError::kind()` maps each variant.
- **Deliverable:** the contract compiles and is exercised end-to-end over the mock harness.

### PR 3 — `AuthSource` + `Auth` layer + `Guarded` body

- **`AuthSource`** (`src/auth.rs`) — `trait AuthSource: Clone + Send + Sync { fn authorize(&self, req: &mut http::Request<Bytes>) -> impl Future<Output = Result<(), HttpError>> + Send; }`; `NoAuth` (ready `Ok(())`); the `Auth` layer (`self.auth.authorize(&mut req).await?; self.inner.call(req).await`). `SetHeaders` sits just outside `Auth` (precedence pinned).
- **`Guarded<B>`** (`src/body.rs`, add `async-lock` dep) — `struct Guarded<B> { #[pin] inner: B, permit: Option<async_lock::SemaphoreGuardArc> }`; `Body` impl forwards `is_end_stream`/`size_hint` and `take()`s the permit on the terminal frame (eager release).
- **Tests:** `authorize` runs once per attempt with a fresh value (recording `MockClient`); `NoAuth` is ready-`Ok`; an `authorize` error surfaces as `ErrorKind::Auth`; `Guarded` parity; eager release on terminal frame **and** on early drop (over a real `async_lock::Semaphore` + a `MockBody`).
- **Deliverable:** `Auth` over `MockClient` and `Guarded` correctness, both green.

### PR 4 — `RateKey` + `RateLimitConfig` + boot-time coverage

- **`RateKey`** (`src/rate.rs`) — `trait RateKey: Hash + Eq + Clone + Send + Sync + 'static { fn all() -> &'static [Self] where Self: Sized; }`.
- **Config types** (`src/rate.rs`) — `LimitPolicy { TokenBucket { rate, burst }, Concurrency { max } }`; `LimitDecl { Policy(LimitPolicy), GlobalOnly }`; `RateLimitConfig<K> { global: LimitPolicy, local: HashMap<K, LimitDecl> }`; `BuildError` (`thiserror`: `UndeclaredKey`, bad-policy-params, missing-global).
- **`validate_coverage`** — the construction-time check: `local` total over `K::all()`, `global` present, param sanity (`rate > 0`, `burst >= 1`, `max >= 1`) → `Result<(), BuildError>`. (Wired into `stack()`/`build()` in Slice 2; unit-tested standalone here.)
- **Tests:** a config missing a `K` variant → `Err(BuildError::UndeclaredKey)`; a total config → `Ok`; bad params / missing global → `Err`. Uses a test `RateKey` enum whose `all()` is guarded by an exhaustive-`match` test (drift-proofing).
- **Deliverable:** the coverage validator is correct and ready for Slice 2's `stack()`/`build()` to call.

---

## Forward notes (deferred — recorded, not for Slice 0)

- **Read `now()` once per request and thread it down.** The timing layers each call `Timer::now()` independently (`clock_gettime(CLOCK_MONOTONIC)` via the vDSO, ~15–25 ns — negligible at IBKR's ≤10 req/s). If a high-throughput venue ever lands, the cheap win is one `now()` read threaded through the stack, and/or a TSC-backed production `Timer` impl (`quanta`) — both behind the `Timer` seam, so zero churn to the layers. Do **not** do either now (YAGNI).

## Self-Review

**Spec coverage (PR 1 scope — the ADR-0029 foundation the construction surface assumes):**
- Repartition `Service` → `net-http-api` ✅ Tasks 1.2–1.3.
- Kernel keeps composition + `ErrorKind`, gains `Timer`, goes std-only ✅ Tasks 1.1, 1.3.
- README graph updated (ADR-0029 Consequences) ✅ Task 1.4.
- The spec's *seam* requirements (AuthSource, Guarded, RateKey/coverage) are **out of PR 1 scope by design** — they are PRs 2–4, roadmapped above, each its own plan. This matches the writing-plans Scope Check (the spec spans multiple subsystems → multiple plans).

**Placeholder scan:** No `TBD`/`TODO`/"handle edge cases" in PR 1 tasks; every code step shows the actual content. The PR 2–4 roadmap is explicitly a roadmap (scope + interfaces), not placeholder tasks.

**Type consistency:** `Timer` signature identical in Task 1.1 and the kernel re-export; `Service` moved verbatim (Task 1.2) and removed at the source (Task 1.3) — one definition after the PR; module rename `service` → `compose` applied consistently in `lib.rs`, the doctest import paths, and the re-export list.
