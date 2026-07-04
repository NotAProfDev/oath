# net-http `CircuitBreaker` Layer (Slice 1, PR 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `CircuitBreaker<S, T>` HTTP middleware layer — the **reactive** backstop to `RateLimit`'s proactive guard — that trips **Open** after `failure_threshold` consecutive transport failures (or immediately on a `Throttled`/429), fast-rejects with a non-retryable `HttpError::CircuitOpen` without touching the inner stack, and after a `Timer`-measured cooldown admits bounded **Half-Open** probes to test recovery.

**Architecture:** A `Timer`-generic, runtime-neutral `Service` wrapper in `oath-adapter-net-http-api`. The highest-consequence logic — the Closed/Open/Half-Open state machine — lives in a **pure, clock-injected `Breaker`** (`admit(now) -> Admit`, `record(class, now)`), table-testable with zero async. The `CircuitBreaker<S, T>` shell is a thin `Arc<Mutex<Breaker>>` + `Timer`: it locks briefly to `admit` (using `timer.now()`), **releases the lock**, runs `inner.call(req).await` (or returns `CircuitOpen` on rejection), then locks briefly to `record` the `classify`-d outcome. The breaker **never sleeps** — Open→Half-Open is a lazy `now()` comparison on the next `admit` — so there is no `futures-util` race, no lock held across an `await`, and no new dependency. Body-transparent: `http::Response<B>` is forwarded untouched (the breaker only reads `status()`).

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `http`/`bytes`, `std::time::{Duration, Instant}`, `std::num::NonZeroU32`, `std::sync::{Arc, Mutex}`, `net-api::{Timer, ErrorKind, HasErrorKind, Layer}`. Tests use inline service doubles + `MockTimer` (`oath-adapter-net-mock`), driven on `tokio` (dev-only). **No new dependency**; the `net-api` contract crate gains one `ErrorKind` variant.

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result` or use total combinators. Recover a poisoned `std::sync::Mutex` with `.unwrap_or_else(std::sync::PoisonError::into_inner)`, **never** `.lock().unwrap()`. Counter increments use `saturating_add`. Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations` — hand-impl where a derive would demand `Debug`/`Clone` on `S`/`T`), `const fn` where `missing_const_for_fn` asks (but **not** `CircuitBreakerLayer::new` — it allocates an `Arc`; see Task 4).
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde` in non-dev deps. **This PR adds no dependency** (`http`/`bytes`/`http-body`/`async-lock` are crate deps; `oath-adapter-net-mock` + `tokio` are dev-deps — all present since #76/#78), so `cargo-deny`/`machete` are unaffected. The `net-api` change is a single enum variant, no new dep there either.
- **net-http-api tests must NOT dev-depend on `oath-adapter-net-http-mock` (`MockClient`)** — it normal-depends on this crate, so the dev-dep closes a cycle that recompiles a second, non-unifying copy of `net-http-api` (E0599: `MockClient` does not satisfy *this* crate's `Service`). Use **inline** service doubles + `oath-adapter-net-mock`'s `MockTimer`, exactly as `rate_limit.rs`/`timeout.rs`/`retry.rs`/`body.rs` do.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-04-net-http-circuitbreaker-layer-design.md](../specs/2026-07-04-net-http-circuitbreaker-layer-design.md), governed by [ADR-0031 §5](../../adr/0031-http-resilience-venue-pacing.md) and [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 1, PR 4** — the fourth of the resilience-layer PRs (RateLimit #76, Timeout #78, Retry #82 landed; then CircuitBreaker, Tracing).

## File Structure

- `crates/adapter/net/api/src/error_kind.rs` — **modify** (Task 1). Add the `ErrorKind::CircuitOpen` variant.
- `crates/adapter/net/http/api/src/error.rs` — **modify** (Task 1). Add the `HttpError::CircuitOpen` variant + its `HasErrorKind` arm + the mapping-test row.
- `crates/adapter/net/http/api/src/circuit_breaker.rs` — **new** (Tasks 2–4). `CircuitBreakerConfig`, `Class`/`classify`, `BreakerState`/`Admit`/`Breaker`, `CircuitBreakerLayer<T>`, `CircuitBreaker<S, T>`, the `Layer`/`Service` impls, and their tests.
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Tasks 2, 4). `pub mod circuit_breaker;` + re-exports + module-doc bullet.
- `docs/adr/0034-...md`, `CHANGELOG.md` — **modify** (Task 5).

No `Cargo.toml` change. Each task is one commit; the tasks together are one PR/issue.

---

## Setup: issue (worktree already exists)

> The isolated worktree **already exists** at `.claude/worktrees/net-http-circuit-breaker` (branch `feat/net-http-circuit-breaker`, branched off `main` = `68d8f60`, which carries #76/#78/#80/#82). The design-spec commit (`29b7203`) is already on the branch. All tasks run inside the worktree. Only the GitHub issue remains to be created.

- [ ] **Create the issue**

```bash
gh issue create \
  --title "feat(net): CircuitBreaker resilience layer (Slice 1, PR 4)" \
  --label enhancement \
  --body "Slice 1 PR 4 of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-circuitbreaker-layer-design.md; ADR-0031 §5).

- \`CircuitBreaker<S, T>\` + \`CircuitBreakerLayer<T>\` (impl \`net-api::Layer\`): the reactive backstop to RateLimit — trips Open after \`failure_threshold\` consecutive Connection/Timeout/5xx failures, or immediately on Throttled/429 with the long \`throttle_cooldown\`; fast-rejects with a new non-retryable \`HttpError::CircuitOpen\` / \`ErrorKind::CircuitOpen\`; lazy Half-Open probing (now()-only timing, no sleep, no new dependency)
- Pure clock-injected \`Breaker\` state machine (Closed/Open/Half-Open), table-tested with zero async; thin \`Arc<Mutex<Breaker>>\` Service shell, single per-host breaker
- 4-class outcome partition (Failure / TripNow / Ignored / Success): 4xx/Auth/unclassified errors neither trip nor mask a building outage; Unknown -> Ignored for v1 (resilience4j fail-safe recorded as a future improvement)
- Body-transparent; sits outside Retry (counts logical post-retry outcomes)"
```

Note the issue number `#<N>` for the PR body.

---

## Task 1: the `CircuitOpen` error surface

**Files:**
- Modify: `crates/adapter/net/api/src/error_kind.rs`
- Modify: `crates/adapter/net/http/api/src/error.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `oath_adapter_net_api::ErrorKind::CircuitOpen` — a new variant on the `#[non_exhaustive]` enum: a local fast-reject, distinct from a transport failure.
  - `oath_adapter_net_http_api::HttpError::CircuitOpen` — a new variant (no source), `kind() -> ErrorKind::CircuitOpen`. Consumed by Task 4's Service shell.

- [ ] **Step 1: Write the failing test**

In `crates/adapter/net/http/api/src/error.rs`, extend the existing `kind_maps_each_variant` test with a `CircuitOpen` row (add this line inside that `#[test]` fn, after the `other` assertion):

```rust
        assert_eq!(HttpError::CircuitOpen.kind(), ErrorKind::CircuitOpen);
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `no variant named CircuitOpen found for enum HttpError` (and, once that is added, `ErrorKind`).

- [ ] **Step 3: Add both variants + the mapping**

In `crates/adapter/net/api/src/error_kind.rs`, add the variant **after** `Unknown` (the enum is already `#[non_exhaustive]`, so downstream crates need no change):

```rust
    /// The error does not fit any other category.
    Unknown,

    /// A circuit breaker rejected the request without sending it — the breaker is
    /// Open after prior failures (or a throttle) and is failing fast until its
    /// cooldown elapses. A deliberate local decision, not a transport outcome;
    /// non-retryable.
    CircuitOpen,
```

In `crates/adapter/net/http/api/src/error.rs`, add the `HttpError` variant (after `Other`, keeping the `#[non_exhaustive]` enum's `thiserror` style):

```rust
    /// The circuit breaker is open — the request was rejected without being sent.
    CircuitOpen,
```

with its `#[error(...)]` message directly above it:

```rust
    #[error("circuit open: request rejected without being sent")]
    CircuitOpen,
```

and add the `HasErrorKind` arm (in the `match self` inside `fn kind`, after the `Other` arm):

```rust
            Self::CircuitOpen => ErrorKind::CircuitOpen,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-api && cargo test -p oath-adapter-net-http-api error && just lint`
Expected: PASS, warning-free.

> Known risks:
> - Adding a variant to the `#[non_exhaustive]` `ErrorKind` can surface a **non-exhaustive `match`** in this or another crate that lacked a `_` arm. If `just check` reports one, add the missing arm — a `Class::Ignored`-equivalent "not a failure/trip" default where the match is a breaker/retry classifier, or the locally-correct value otherwise. (`retry.rs::is_transient` uses `matches!`, which is unaffected.)
> - `HttpError` is `#[non_exhaustive]`; its own `HasErrorKind` `match self` **is** exhaustive over `HttpError` and now covers the new arm.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/api/src/error_kind.rs crates/adapter/net/http/api/src/error.rs
git commit -m "feat(net): add CircuitOpen error kind + HttpError::CircuitOpen"
```

---

## Task 2: `CircuitBreakerConfig` + `Class`/`classify` + module scaffold

**Files:**
- Create: `crates/adapter/net/http/api/src/circuit_breaker.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `HttpError` (crate); `ErrorKind`, `HasErrorKind` (`oath_adapter_net_api`).
- Produces:
  - `oath_adapter_net_http_api::CircuitBreakerConfig` — `struct { failure_threshold: NonZeroU32, cooldown: Duration, throttle_cooldown: Duration, half_open_probes: NonZeroU32 }` (`Debug`, `Clone`, `Copy`). Consumed by Tasks 3–4.
  - `Class` (crate-private) — `enum { Failure, TripNow, Ignored, Success }` (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`). Consumed by Task 3's `Breaker::record`.
  - `classify` (crate-private) — `fn classify<B>(&Result<http::Response<B>, HttpError>) -> Class`. Consumed by Task 4's Service.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/circuit_breaker.rs` with the module doc, the config, the classifier, and its table test:

```rust
//! The `CircuitBreaker` resilience layer (ADR-0031 §5): the reactive 429/outage
//! backstop to `RateLimit`'s proactive pacing.
//!
//! `RateLimit` tries never to hit a 429; `CircuitBreaker` stops cold if the host
//! fails anyway. It trips **Open** after [`CircuitBreakerConfig::failure_threshold`]
//! consecutive transport failures (`HttpError::{Connection, Timeout}` or a `5xx`
//! response), or **immediately** on a `Throttled`/429 with the long
//! [`CircuitBreakerConfig::throttle_cooldown`] (IBKR's ~15-minute penalty box).
//! While Open it **fast-rejects** every request with a non-retryable
//! [`HttpError::CircuitOpen`](crate::HttpError::CircuitOpen) — the inner stack is
//! never touched. After the cooldown a bounded number of **Half-Open** probes test
//! recovery: a reached-host response closes the circuit, a failure re-opens it.
//!
//! The state machine lives in a pure, clock-injected [`Breaker`] (transitions take
//! `now: Instant` as an input, table-tested with zero async); the [`CircuitBreaker`]
//! service is a thin `Arc<Mutex<Breaker>>` + [`Timer`](oath_adapter_net_api::Timer)
//! shell. A **single per-host** breaker is shared behind `Arc`. Runtime-neutral and
//! `now()`-only — the breaker never sleeps (Open→Half-Open is a lazy comparison on
//! the next admit), so there is no timer race and no new dependency. Body-transparent
//! — `http::Response<B>` is forwarded untouched.

use crate::HttpError;
use oath_adapter_net_api::{ErrorKind, HasErrorKind};
use std::num::NonZeroU32;
use std::time::Duration;

/// The circuit breaker's thresholds, as plain `Copy` data (ADR-0031 §5).
///
/// `failure_threshold` and `half_open_probes` are `NonZeroU32`: "≥ 1" is a type
/// invariant, so [`CircuitBreakerLayer::new`](crate::CircuitBreakerLayer) needs no
/// `Result` (a `0` threshold is nonsense and `0` probes would leave a tripped
/// circuit stuck Open forever). This types §5's `u32` sketch more precisely.
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures in the Closed state that trip the circuit Open.
    pub failure_threshold: NonZeroU32,
    /// The cooldown before Half-Open probing after a failure-threshold trip.
    pub cooldown: Duration,
    /// The (longer) cooldown after a `Throttled`/429 trip — the penalty box.
    pub throttle_cooldown: Duration,
    /// Probes admitted per Half-Open episode; all must reach the host to close.
    pub half_open_probes: NonZeroU32,
}

/// The breaker-relevant classification of one call outcome (pure, state-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// A genuine transport/server failure — advances the Closed trip counter.
    Failure,
    /// A throttle/429 — trips the circuit **immediately** on the long cooldown.
    TripNow,
    /// Neither a failure nor a trip (4xx, `Auth`, unclassified) — a no-op in Closed;
    /// resolves a Half-Open probe (a reached host proves recovery).
    Ignored,
    /// A healthy `2xx`/`3xx` response — resets the streak / resolves a probe.
    Success,
}

/// Classify a call outcome for the breaker (ADR-0031 §5).
///
/// Only genuine transport failures (`Connection`/`Timeout`) and `5xx` are
/// `Failure`; `Throttled`/429 is `TripNow`; a `4xx`/`Auth`/unclassified error is
/// `Ignored` (never trips **and never resets** — so an interleave cannot mask a
/// building outage); `2xx`/`3xx` is `Success`. `Unknown → Ignored` is the
/// conservative v1 default (the resilience4j fail-safe `Unknown → Failure` is a
/// documented future improvement).
pub(crate) fn classify<B>(outcome: &Result<http::Response<B>, HttpError>) -> Class {
    match outcome {
        Err(e) => match e.kind() {
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Server => Class::Failure,
            ErrorKind::Throttled => Class::TripNow,
            // Auth, Client, Unknown, CircuitOpen — and any future kind — are Ignored.
            _ => Class::Ignored,
        },
        Ok(resp) => {
            let s = resp.status();
            if s == http::StatusCode::TOO_MANY_REQUESTS {
                Class::TripNow
            } else if s.is_server_error() {
                Class::Failure
            } else if s.is_client_error() {
                Class::Ignored
            } else {
                Class::Success
            }
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::{Class, classify};
    use crate::HttpError;

    fn ok(status: u16) -> Result<http::Response<()>, HttpError> {
        let mut r = http::Response::new(());
        *r.status_mut() = http::StatusCode::from_u16(status).unwrap();
        Ok(r)
    }

    #[test]
    fn transport_errors_and_5xx_are_failures() {
        assert_eq!(classify::<()>(&Err(HttpError::Timeout)), Class::Failure);
        assert_eq!(classify::<()>(&Err(HttpError::connection("reset"))), Class::Failure);
        assert_eq!(classify(&ok(500)), Class::Failure);
        assert_eq!(classify(&ok(503)), Class::Failure);
    }

    #[test]
    fn throttle_and_429_trip_now() {
        assert_eq!(classify::<()>(&Err(HttpError::Throttled)), Class::TripNow);
        assert_eq!(classify(&ok(429)), Class::TripNow);
    }

    #[test]
    fn client_errors_auth_and_unknown_are_ignored() {
        assert_eq!(classify(&ok(400)), Class::Ignored);
        assert_eq!(classify(&ok(404)), Class::Ignored);
        assert_eq!(classify::<()>(&Err(HttpError::auth("expired"))), Class::Ignored);
        assert_eq!(classify::<()>(&Err(HttpError::other("boom"))), Class::Ignored);
    }

    #[test]
    fn success_statuses_are_success() {
        assert_eq!(classify(&ok(200)), Class::Success);
        assert_eq!(classify(&ok(301)), Class::Success);
    }
}
```

In `lib.rs`, add the module-doc bullet (insert **after** the `retry` bullet, before the `timeout` bullet), the `pub mod` (insert **after** `pub mod body;`, before `pub mod client;` — alphabetical), and the re-export (insert **after** the `body::{…}` re-export, before `pub use client::HttpClient;`):

Module-doc bullet:

```rust
//! - [`circuit_breaker`] — the `CircuitBreaker` layer, its `CircuitBreakerLayer`
//!   factory, and the `CircuitBreakerConfig` thresholds
```

Module declaration:

```rust
pub mod circuit_breaker;
```

Re-export:

```rust
pub use circuit_breaker::CircuitBreakerConfig;
```

(Task 4 extends this to `pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerLayer};`.)

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: initially FAIL if the module wiring is added before the file exists; once `circuit_breaker.rs` and the `lib.rs` wiring are both in place it compiles. The `classify_tests` are the real gate.

- [ ] **Step 3: (implementation already written in Step 1)**

`CircuitBreakerConfig`, `Class`, and `classify` are fully defined in Step 1 — there is no separate implementation step for this task.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker && just lint`
Expected: PASS, warning-free.

> Known risks:
> - `classify::<()>(&Err(...))` needs the turbofish because `B` is otherwise unconstrained on the `Err` arm; the `ok(...)` helper pins `B = ()` on the `Ok` arm.
> - If clippy `missing_docs` fires, note `Class`/`classify` are `pub(crate)` (no doc required) but are documented anyway; `CircuitBreakerConfig` and its fields are `pub` and **must** be documented (they are).

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): CircuitBreakerConfig + outcome classifier for the breaker"
```

---

## Task 3: the pure `Breaker` state machine

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs`

**Interfaces:**
- Consumes: `CircuitBreakerConfig`, `Class` (Task 2).
- Produces (crate-private — **not** re-exported):
  - `Admit` — `enum { Pass, Reject }` (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`).
  - `Breaker` — `struct` with `const fn new(cfg: CircuitBreakerConfig) -> Self`, `fn admit(&mut self, now: Instant) -> Admit`, `fn record(&mut self, class: Class, now: Instant)`. Task 4's shell owns one behind `Arc<Mutex<…>>`.

- [ ] **Step 1: Write the failing tests**

Append a **new** `#[cfg(test)]` module below `classify_tests` in `circuit_breaker.rs`:

```rust
#[cfg(test)]
mod breaker_tests {
    use super::{Admit, Breaker, CircuitBreakerConfig, Class};
    use std::num::NonZeroU32;
    use std::time::{Duration, Instant};

    fn cfg(threshold: u32, probes: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown: Duration::from_secs(30),
            throttle_cooldown: Duration::from_secs(900),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }

    #[test]
    fn closed_trips_after_threshold_consecutive_failures() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        assert_eq!(b.admit(now), Admit::Pass);
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Pass, "still closed after 2 failures");
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Reject, "3rd consecutive failure → Open rejects");
    }

    #[test]
    fn a_success_resets_the_failure_streak() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        b.record(Class::Success, now); // reset
        b.record(Class::Failure, now);
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Pass, "streak reset → not tripped");
    }

    #[test]
    fn ignored_does_not_reset_the_streak() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::Failure, now);
        b.record(Class::Ignored, now); // a 4xx does NOT reset — anti-masking
        b.record(Class::Failure, now);
        b.record(Class::Failure, now); // 3rd failure overall → trips
        assert_eq!(b.admit(now), Admit::Reject, "ignored left the streak intact → trips");
    }

    #[test]
    fn throttle_trips_immediately_on_the_long_cooldown() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(3, 1));
        b.record(Class::TripNow, now); // one throttle → Open, no threshold needed
        assert_eq!(b.admit(now), Admit::Reject);
        assert_eq!(
            b.admit(now + Duration::from_secs(30)),
            Admit::Reject,
            "the short cooldown is insufficient for a throttle trip"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(900)),
            Admit::Pass,
            "throttle_cooldown elapsed → first probe admitted"
        );
    }

    #[test]
    fn open_rejects_until_cooldown_then_admits_one_probe() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // trips on the first failure
        b.record(Class::Failure, now);
        assert_eq!(b.admit(now), Admit::Reject);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass, "cooldown elapsed → first probe");
        assert_eq!(b.admit(after), Admit::Reject, "concurrency gate: no 2nd probe");
    }

    #[test]
    fn half_open_probe_success_closes() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Success, after);
        assert_eq!(b.admit(after), Admit::Pass, "probe succeeded → closed");
    }

    #[test]
    fn half_open_probe_ignored_also_closes() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Ignored, after); // a 4xx probe still proves the host is reachable
        assert_eq!(b.admit(after), Admit::Pass, "ignored probe → closed (no stuck half-open)");
    }

    #[test]
    fn half_open_probe_failure_reopens() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass);
        b.record(Class::Failure, after); // probe fails → reopen with a fresh cooldown
        assert_eq!(b.admit(after), Admit::Reject, "re-opened");
        assert_eq!(
            b.admit(after + Duration::from_secs(30)),
            Admit::Pass,
            "the fresh cooldown runs from the failed probe"
        );
    }

    #[test]
    fn multi_probe_half_open_requires_all_to_close() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 2)); // 2 probes per episode
        b.record(Class::Failure, now);
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Pass, "probe 1");
        assert_eq!(b.admit(after), Admit::Pass, "probe 2");
        assert_eq!(b.admit(after), Admit::Reject, "no probe 3 (gate)");
        b.record(Class::Success, after); // 1 of 2
        assert_eq!(b.admit(after), Admit::Reject, "still half-open, awaiting the 2nd");
        b.record(Class::Success, after); // 2 of 2 → close
        assert_eq!(b.admit(after), Admit::Pass, "both probes reached → closed");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Breaker`/`Admit` in module `circuit_breaker`.

- [ ] **Step 3: Implement the state machine**

Extend the top-of-file `use` block with `Instant`:

```rust
use std::time::{Duration, Instant};
```

(Replace the existing `use std::time::Duration;` line from Task 2 with the combined import above — keep a single copy.)

Insert the types **between** the `classify` fn and the `classify_tests` module:

```rust
/// The breaker's state (ADR-0031 §5). `Instant` deadlines are compared against
/// `Timer::now()` by the async shell — the core itself never reads a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState {
    /// Passing requests; `consecutive_failures` counts toward the trip threshold.
    Closed { consecutive_failures: u32 },
    /// Rejecting fast until `reopen_at`; then the next admit begins Half-Open.
    Open { reopen_at: Instant },
    /// Probing: `probes_left` may still be admitted, `successes_needed` must reach
    /// the host before the circuit closes.
    HalfOpen { probes_left: u32, successes_needed: u32 },
}

/// The admission verdict for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Admit the call to the inner stack.
    Pass,
    /// Reject the call fast with `CircuitOpen` — the inner stack is not touched.
    Reject,
}

/// The pure circuit-breaker state machine (ADR-0031 §5).
///
/// Clock-injected: every transition takes `now: Instant` as an input, so the whole
/// unit is table-testable with zero async. The async [`CircuitBreaker`] shell owns
/// the `Mutex` and the `Timer`; this type holds neither.
#[derive(Debug, Clone)]
pub(crate) struct Breaker {
    state: BreakerState,
    cfg: CircuitBreakerConfig,
}

impl Breaker {
    /// A fresh breaker starts Closed with no failures.
    pub(crate) const fn new(cfg: CircuitBreakerConfig) -> Self {
        Self {
            state: BreakerState::Closed {
                consecutive_failures: 0,
            },
            cfg,
        }
    }

    /// Decide whether to admit a call now, transitioning Open→Half-Open lazily.
    pub(crate) fn admit(&mut self, now: Instant) -> Admit {
        match &mut self.state {
            BreakerState::Closed { .. } => Admit::Pass,
            BreakerState::Open { reopen_at } => {
                if now >= *reopen_at {
                    // Cooldown elapsed → begin a Half-Open episode; THIS call is the
                    // first probe (so `probes_left` starts one short of the budget).
                    let probes = self.cfg.half_open_probes.get();
                    self.state = BreakerState::HalfOpen {
                        probes_left: probes - 1,
                        successes_needed: probes,
                    };
                    Admit::Pass
                } else {
                    Admit::Reject
                }
            }
            BreakerState::HalfOpen { probes_left, .. } => {
                if *probes_left > 0 {
                    *probes_left -= 1;
                    Admit::Pass
                } else {
                    Admit::Reject // concurrency gate: no more than `half_open_probes` in flight
                }
            }
        }
    }

    /// Record a classified outcome, transitioning as ADR-0031 §5 dictates.
    pub(crate) fn record(&mut self, class: Class, now: Instant) {
        match self.state {
            BreakerState::Closed {
                consecutive_failures,
            } => match class {
                Class::Failure => {
                    let n = consecutive_failures.saturating_add(1);
                    self.state = if n >= self.cfg.failure_threshold.get() {
                        BreakerState::Open {
                            reopen_at: now + self.cfg.cooldown,
                        }
                    } else {
                        BreakerState::Closed {
                            consecutive_failures: n,
                        }
                    };
                }
                Class::TripNow => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.throttle_cooldown,
                    };
                }
                Class::Ignored => {} // streak untouched — a 4xx/Auth neither trips nor resets
                Class::Success => {
                    self.state = BreakerState::Closed {
                        consecutive_failures: 0,
                    };
                }
            },
            BreakerState::HalfOpen {
                probes_left,
                successes_needed,
            } => match class {
                Class::Failure => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.cooldown,
                    };
                }
                Class::TripNow => {
                    self.state = BreakerState::Open {
                        reopen_at: now + self.cfg.throttle_cooldown,
                    };
                }
                // A reached-host probe (2xx/3xx or 4xx/Auth) resolves; the last one closes.
                Class::Ignored | Class::Success => {
                    self.state = if successes_needed <= 1 {
                        BreakerState::Closed {
                            consecutive_failures: 0,
                        }
                    } else {
                        BreakerState::HalfOpen {
                            probes_left,
                            successes_needed: successes_needed - 1,
                        }
                    };
                }
            },
            // A stale outcome from a call admitted before a concurrent trip; drop it.
            // Never un-trips a freshly-opened circuit (single global v1 breaker).
            BreakerState::Open { .. } => {}
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api circuit_breaker && just lint && just doc`
Expected: PASS, warning-free.

> Known risks:
> - **`now + self.cfg.cooldown`** is `Instant + Duration`. A config cooldown so large it overflows `Instant` is not a real config; if clippy/overflow is a concern, it panics only in debug on an absurd (~292-billion-year) duration — acceptable, matching how `MockTimer`/`RateLimit` add `Duration` to `Instant`. Do **not** introduce `checked_add` here unless a test demands it (it would force an `Option` with no sensible fallback).
> - **`probes - 1` / `successes_needed - 1`** never underflow: `admit` only enters the `probes - 1` branch with `probes = half_open_probes.get() ≥ 1`, and the `successes_needed - 1` branch is guarded by `successes_needed <= 1 → close` (so the `else` has `successes_needed ≥ 2`). If clippy `arithmetic_side_effects` (nursery) flags them, they are provably safe; prefer a clarifying comment over `saturating_sub` (which would hide a logic bug).
> - **`consecutive_failures.saturating_add(1)`** guards the degenerate `failure_threshold = u32::MAX` case with no panic.
> - `Breaker`/`BreakerState`/`Admit` are `pub(crate)` — no `missing_docs`/`missing_debug_implementations` obligation, but they carry docs + `Debug` anyway.
> - **`#[allow(dead_code)]` on the new pure items.** Like Task 2's `Class`/`classify`, the lib target sees `Breaker`/`BreakerState`/`Admit` and their methods as unused (they are consumed only by `breaker_tests` and by Task 4's Service), so `just lint`'s `--all-targets` scope fails with `dead_code`. Add `#[allow(dead_code)]` where needed to keep `just lint` green; Task 4 deletes them all once the Service references them in non-test code.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs
git commit -m "feat(net): pure clock-injected Breaker state machine (Closed/Open/Half-Open)"
```

---

## Task 4: `CircuitBreaker` layer — the async shell

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `CircuitBreakerConfig`, `Breaker`, `Admit`, `classify` (Tasks 2–3); `HttpError`, `Service` (crate); `Layer`, `Timer` (`oath_adapter_net_api`).
- Produces:
  - `oath_adapter_net_http_api::CircuitBreakerLayer<T>` — `impl Layer<S>` factory; `pub fn new(cfg: CircuitBreakerConfig, timer: T) -> Self` (**infallible**; constructs the single shared `Arc<Mutex<Breaker>>`).
  - `oath_adapter_net_http_api::CircuitBreaker<S, T>` — the wrapping `Service`; for an inner `S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync` and `T: Timer`, it is `Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>` (body-transparent — same `B`, no `B: Body` bound).

- [ ] **Step 1: Write the failing tests**

Append a **new** `#[cfg(test)]` module below `breaker_tests` in `circuit_breaker.rs`:

```rust
#[cfg(test)]
mod service_tests {
    use super::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerLayer};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use oath_adapter_net_api::{ErrorKind, Layer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // One scripted outcome per attempt. `Copy` so the leaf reads it by index.
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

    // An inline leaf yielding a scripted sequence of outcomes, counting calls. Once
    // the script is exhausted it repeats the last step. Body is `()` — the breaker
    // only reads `status()`, never the body. Inline (not `MockClient`) to avoid the
    // net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
    }
    impl ScriptLeaf {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: Arc::new(steps),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<()>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self
                .steps
                .get(i)
                .copied()
                .unwrap_or_else(|| *self.steps.last().unwrap());
            async move {
                match step {
                    Step::Err(kind) => Err(err_of(kind)),
                    Step::Status(code) => {
                        let mut resp = http::Response::new(());
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    }
                }
            }
        }
    }

    fn cfg(threshold: u32, cooldown: Duration, throttle: Duration, probes: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown,
            throttle_cooldown: throttle,
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn bare_req() -> http::Request<Bytes> {
        http::Request::new(Bytes::new())
    }

    #[tokio::test]
    async fn trips_after_threshold_then_fast_rejects_without_touching_the_leaf() {
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]); // always fails
        let svc = CircuitBreakerLayer::new(cfg(3, secs(30), secs(900), 1), MockTimer::new())
            .layer(leaf.clone());
        for _ in 0..3 {
            let _ = svc.call(bare_req()).await; // 3 consecutive failures trip it
        }
        assert_eq!(leaf.calls(), 3);
        let err = svc.call(bare_req()).await.unwrap_err();
        assert!(matches!(err, HttpError::CircuitOpen));
        assert_eq!(leaf.calls(), 3, "an open circuit fast-rejects; the leaf is untouched");
    }

    #[tokio::test]
    async fn a_single_429_trips_immediately_on_the_long_cooldown() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::Status(429), Step::Status(200)]);
        let svc = CircuitBreakerLayer::new(cfg(3, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let resp = svc.call(bare_req()).await.expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert!(
            matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen),
            "one 429 trips the circuit"
        );
        timer.advance(secs(30)); // the SHORT cooldown is not enough for a throttle trip
        assert!(matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen));
        timer.advance(secs(900)); // now past throttle_cooldown
        let resp = svc.call(bare_req()).await.expect("probe admitted, leaf returns 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2, "one 429 + one probe; the fast-rejects never hit the leaf");
    }

    #[tokio::test]
    async fn recovers_when_the_cooldown_probe_succeeds() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Timeout),
            Step::Err(ErrorKind::Timeout),
            Step::Status(200),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await; // fail 1
        let _ = svc.call(bare_req()).await; // fail 2 → Open
        assert!(matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen));
        timer.advance(secs(30));
        let ok = svc.call(bare_req()).await.expect("probe hits the leaf → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
        let ok2 = svc.call(bare_req()).await.expect("closed → next call flows");
        assert_eq!(ok2.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 4, "2 failures + 2 post-recovery sends; rejects skip the leaf");
    }

    #[tokio::test]
    async fn reopens_when_the_cooldown_probe_fails() {
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Connection),
            Step::Err(ErrorKind::Connection),
            Step::Status(503),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await;
        let _ = svc.call(bare_req()).await; // Open
        assert!(matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen));
        timer.advance(secs(30));
        let resp = svc.call(bare_req()).await.expect("probe returns a 503 as Ok");
        assert_eq!(resp.status(), 503);
        assert!(
            matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen),
            "the probe failed → re-opened"
        );
        assert_eq!(leaf.calls(), 3);
    }

    #[tokio::test]
    async fn clones_from_one_layer_share_the_breaker() {
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]);
        let layer = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), MockTimer::new());
        let a = layer.layer(leaf.clone());
        let b = a.clone(); // shares the Arc<Mutex<Breaker>>
        let _ = a.call(bare_req()).await; // fail 1 via A
        let _ = a.call(bare_req()).await; // fail 2 via A → Open
        assert!(
            matches!(b.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen),
            "clone B observes A's trip (single per-host breaker)"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type CircuitBreaker`/`CircuitBreakerLayer` in module `circuit_breaker`.

- [ ] **Step 3: Implement the shell**

Extend the top-of-file `use` block (merge with the Task 2/3 imports — keep one copy of each):

```rust
use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
```

Insert the layer + service **between** the `Breaker` impl (Task 3) and the `classify_tests` module:

```rust
/// The `CircuitBreaker` [`Layer`] factory: holds the single shared breaker + clock.
///
/// `new` constructs the breaker **once** into an `Arc<Mutex<…>>`; every service it
/// produces (and every clone) shares it — a single per-host breaker (ADR-0031 §5).
pub struct CircuitBreakerLayer<T> {
    breaker: Arc<Mutex<Breaker>>,
    timer: T,
}

impl<T> CircuitBreakerLayer<T> {
    /// Build the layer from thresholds and a [`Timer`] clock.
    ///
    /// **Infallible** — `NonZeroU32` makes the two counts "≥ 1" a type invariant
    /// (contrast `RateLimitLayer::new`, which validates a config map). Not `const`:
    /// it allocates the shared `Arc<Mutex<Breaker>>`.
    #[must_use]
    pub fn new(cfg: CircuitBreakerConfig, timer: T) -> Self {
        Self {
            breaker: Arc::new(Mutex::new(Breaker::new(cfg))),
            timer,
        }
    }
}

impl<T: Clone> Clone for CircuitBreakerLayer<T> {
    fn clone(&self) -> Self {
        Self {
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for CircuitBreakerLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreakerLayer").finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for CircuitBreakerLayer<T> {
    type Service = CircuitBreaker<S, T>;

    fn layer(&self, inner: S) -> CircuitBreaker<S, T> {
        CircuitBreaker {
            inner,
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

/// The `CircuitBreaker` middleware: fast-rejects while Open, else forwards.
///
/// A thin shell over the pure [`Breaker`]: it locks briefly to `admit` (using
/// `timer.now()`), releases the lock, runs `inner.call` (or returns `CircuitOpen`),
/// then locks briefly to `record` the classified outcome. The lock is **never**
/// held across the `await`. Body-transparent — `http::Response<B>` is forwarded.
pub struct CircuitBreaker<S, T> {
    inner: S,
    breaker: Arc<Mutex<Breaker>>,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for CircuitBreaker<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            breaker: Arc::clone(&self.breaker),
            timer: self.timer.clone(),
        }
    }
}

impl<S, T> fmt::Debug for CircuitBreaker<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreaker").finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for CircuitBreaker<S, T>
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
            // Admit decision under a short lock (released at the end of this block).
            let admit = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                breaker.admit(now)
            };
            if let Admit::Reject = admit {
                return Err(HttpError::CircuitOpen); // fast reject — the leaf is not touched
            }

            let outcome = self.inner.call(req).await; // NO lock held across the await

            // Record the classified outcome under a second short lock.
            let class = classify(&outcome);
            {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                breaker.record(class, now);
            }
            outcome
        }
    }
}
```

In `lib.rs`, extend the Task 2 re-export:

```rust
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerLayer};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api circuit_breaker && just lint && just doc`
Expected: PASS, warning-free.

> Known risks:
> - **No `B: Send` bound is needed** (unlike `Retry`): the only `.await` is `inner.call(req)`, and `outcome` is created *after* it with no subsequent await, so `B` never crosses an await point. If a future rustc generator analysis nonetheless demands it, add `B: Send` to the `where` clause (as `Retry`/`RateLimit` carry) — harmless.
> - **`S: Sync`** because the returned `Send` future borrows `&self`; `T: Sync` via `Timer`, and `Arc<Mutex<Breaker>>: Sync`. Same bound the sibling layers carry.
> - **The `Mutex` guard must not cross the `await`** — each `lock()` is confined to its own `{ … }` block that ends before `inner.call(req).await` (admit block) or contains no await (record block). If clippy `await_holding_lock` fires, a guard escaped its block — re-scope it.
> - **`CircuitBreakerLayer::new` is intentionally not `const`** (it calls `Arc::new`). If clippy `missing_const_for_fn` flags it, that is a false positive here — leave it non-`const`.
> - The tests need **no** `spawn`/`yield`/`drain` (contrast `retry.rs`): the breaker never sleeps, so every `svc.call(...).await` resolves synchronously against the leaf; `MockTimer::advance` only moves `now()` for the lazy Open→Half-Open check between calls.
> - **Remove the transitional `#[allow(dead_code)]` allows.** Task 2 put `#[allow(dead_code)]` on `Class`/`classify` and Task 3 put them on `Breaker`/`BreakerState`/`Admit` (and their methods) because the lib target saw them as unused. This task's Service wires `classify`, `Breaker::{new, admit, record}`, `Class`, and `Admit` into non-test code, so those items are now reachable — **delete every `#[allow(dead_code)]`** from `circuit_breaker.rs` and confirm `just lint` stays green (an `allow` of a now-non-firing lint is untidy, though `allow` — unlike `expect` — does not itself warn). Optionally upgrade the module-doc `` `Breaker` ``/`` `CircuitBreaker` `` and the `CircuitBreakerConfig`-doc `` `CircuitBreakerLayer::new` `` from plain code spans to intra-doc links now that the targets exist — then re-run `just doc`.
> - **Run `just doc`** before committing — the module doc links resolve only once these types exist; net-http layer PRs have repeatedly shipped broken rustdoc links that `check`/`lint`/`test` miss.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): CircuitBreaker layer — Arc<Mutex<Breaker>> Service shell over the leaf"
```

---

## Task 5: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Append to ADR-0034's **Amendments (2026-07-04)** numbered list (after item 8, the `Retry` note) a new item 9:

```markdown
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
   *TripNow*; `4xx`/`Auth`/`Unknown` → *Ignored* (never trips **and never resets** —
   so an interleave cannot mask a building outage; an `Auth` error must not trip the
   gateway); `2xx`/`3xx` → *Success*. `failure_threshold`/`half_open_probes` are
   `NonZeroU32` (typing §5's `u32` — "≥ 1" a type invariant, infallible `new`). A
   **single per-host** breaker shared behind `Arc`; **consecutive-count** for v1;
   `now()`-only timing (lazy Open→Half-Open, no sleep, no `futures-util`, no new
   dependency). It sits **outside `Retry`**, counting logical post-retry outcomes.
   Deferred: the resilience4j fail-safe `Unknown → Failure`, rolling-window counting,
   per-key breakers, and a breaker-state observation watch.
```

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added` (after the Retry resilience-layer entry #82):

```markdown
- `oath-adapter-net-http-api` `CircuitBreaker` resilience layer (Slice 1 PR 4) — the
  `CircuitBreaker<S, T>` service + `CircuitBreakerLayer<T>` factory (`net-api::Layer`):
  the reactive backstop to `RateLimit`. Trips Open after `failure_threshold` consecutive
  `Connection`/`Timeout`/`5xx` failures, or immediately on a `Throttled`/429 with the long
  `throttle_cooldown`; fast-rejects with a new non-retryable `HttpError::CircuitOpen`
  (mapped to a new `ErrorKind::CircuitOpen`) without touching the inner stack; admits
  bounded Half-Open probes after cooldown (reached-host closes, failure re-opens). Pure
  clock-injected `Breaker` state machine (Closed/Open/Half-Open) behind a thin
  `Arc<Mutex<Breaker>>` + `Timer` shell; single per-host breaker; `now()`-only (no sleep,
  no new dependency). 4-class outcome partition so `4xx`/`Auth`/unclassified errors neither
  trip nor mask an outage. (ADR-0031 §5, ADR-0034.)
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete — no new dep, so `deny`/`machete` are unaffected).

- [ ] **Step 4: Commit, push, PR**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md
git commit -m "docs(net): record CircuitBreaker layer amendment (ADR-0034 #9) + changelog"
git push -u origin feat/net-http-circuit-breaker
gh pr create \
  --title "feat(net): CircuitBreaker resilience layer (Slice 1, PR 4)" \
  --body "Closes #<N>

Slice 1 **PR 4** of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-circuitbreaker-layer-design.md; ADR-0031 §5). Builds on RateLimit (#76), Timeout (#78), Retry (#82).

- **\`CircuitBreaker<S, T>\`** + **\`CircuitBreakerLayer<T>\`** (\`net-api::Layer\`) — the **reactive** backstop to \`RateLimit\`. Trips **Open** after \`failure_threshold\` consecutive \`Connection\`/\`Timeout\`/\`5xx\` failures, or **immediately** on a \`Throttled\`/429 with the long \`throttle_cooldown\` (IBKR's penalty box); **fast-rejects** with a new non-retryable **\`HttpError::CircuitOpen\`** / **\`ErrorKind::CircuitOpen\`** without touching the inner stack; admits bounded **Half-Open** probes after cooldown (reached-host closes, failure re-opens).
- **Pure clock-injected \`Breaker\`** state machine (Closed/Open/Half-Open), table-tested with zero async; thin **\`Arc<Mutex<Breaker>>\`** + \`Timer\` Service shell, **single per-host** breaker.
- **4-class outcome partition** (Failure / TripNow / Ignored / Success): \`4xx\`/\`Auth\`/unclassified errors neither trip **nor reset** the streak, so an interleave cannot mask a building outage; \`Unknown → Ignored\` for v1 (resilience4j fail-safe recorded as a future improvement).
- **Body-transparent**; sits **outside \`Retry\`** (counts logical post-retry outcomes). \`now()\`-only timing — **no sleep, no \`futures-util\`, no new dependency**; the \`net-api\` contract crate gains one \`ErrorKind\` variant. Recorded as **ADR-0034 Amendment #9**.

MockTimer-driven tests: pure-\`Breaker\` table tests (threshold trip, streak reset, anti-masking, throttle cooldown, half-open close/reopen, concurrency gate) + Service integration (fast-reject with the leaf frozen, immediate-429 trip, cooldown recovery/reopen, shared state across clones).

Next: **Slice 1 PR 5** — the \`Tracing\` layer.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `CircuitBreaker<S, T>` + `CircuitBreakerLayer<T>` (`Layer`), infallible `new`, single-per-host `Arc` sharing — Task 4. ✅
- `CircuitBreakerConfig` (`failure_threshold: NonZeroU32`, `cooldown`, `throttle_cooldown`, `half_open_probes: NonZeroU32`) — Task 2. ✅
- Pure `Breaker`/`BreakerState` core, `admit(now) -> Admit`, `record(class, now)`, the full transition matrix — Task 3 (+ pure table tests). ✅
- 4-class `classify` partition (Failure/TripNow/Ignored/Success; `Unknown → Ignored`) — Task 2 (+ `classify_tests`). ✅
- New `HttpError::CircuitOpen` + new `ErrorKind::CircuitOpen`, non-retryable — Task 1. ✅
- Thin shell: admit under lock → release → `inner.call().await` → record under lock; lock never across await; poison recovered not unwrapped; `now()`-only, no sleep — Task 4. ✅
- Fast-reject leaves the leaf untouched; body-transparent (same `B`, no `B: Body` bound); `S: Sync` — Task 4. ✅
- Sits outside `Retry` (counts logical outcomes) — documented in the module doc + amendment (composition is Slice 2, correctly not built here). ✅
- ADR-0034 Amendment #9 + CHANGELOG — Task 5. ✅
- Deferred (correctly absent): `Unknown → Failure` fail-safe, rolling-window counting, per-key breakers, state-watch, `stack()`/`build()` assembly, `Tracing` — noted, not built. ✅

**Placeholder scan:** none — every step carries actual code or an actual command with expected output (`#<N>` is the PR-time issue number, per house convention; `#82` in the CHANGELOG anchor is the merged Retry PR).

**Type consistency:**
- `CircuitBreakerConfig { failure_threshold: NonZeroU32, cooldown, throttle_cooldown, half_open_probes: NonZeroU32 }` — identical in Task 2's def, Task 3's `Breaker::{new, admit, record}` field reads, and both test `cfg(...)` helpers.
- `Class { Failure, TripNow, Ignored, Success }` — defined Task 2; consumed by Task 3's `record` (all four arms in Closed and Half-Open) and asserted in `classify_tests`.
- `Admit { Pass, Reject }` — defined Task 3; produced by `admit`, matched in Task 4's shell (`if let Admit::Reject`).
- `Breaker::{new(cfg) -> Self, admit(&mut self, Instant) -> Admit, record(&mut self, Class, Instant)}` — defined Task 3, used by Task 4's shell and Task 3's tests.
- `classify::<B>(&Result<http::Response<B>, HttpError>) -> Class` — defined Task 2, called in Task 4's shell (`classify(&outcome)`) and `classify_tests`.
- `CircuitBreakerLayer::new(CircuitBreakerConfig, T) -> Self` + `.layer(inner) -> CircuitBreaker<S, T>` — match the `Interfaces` block and every `service_tests` call.
- `CircuitBreaker` `Service` impl: inner `Response = http::Response<B>` → `Response = http::Response<B>` (transparent) — matches `ScriptLeaf` (`B = ()`).
- `HttpError::CircuitOpen` / `ErrorKind::CircuitOpen` — defined Task 1, produced in Task 4's shell, asserted in Task 1's `kind_maps_each_variant` row and every `service_tests` reject assertion.
- `lib.rs` re-export accumulates to `pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerLayer};`; `Class`/`classify`/`Breaker`/`BreakerState`/`Admit` stay crate-private (not re-exported).

**Known risks to watch during impl:** listed inline — Task 1 Step 4 (non-exhaustive `match` on the widened `#[non_exhaustive]` enum), Task 3 Step 4 (`Instant + Duration`, provably-safe `- 1`, `saturating_add`), Task 4 Step 4 (no `B: Send`, `S: Sync`, `await_holding_lock` re-scoping, non-`const` `new`, no spawn/drain in tests).
