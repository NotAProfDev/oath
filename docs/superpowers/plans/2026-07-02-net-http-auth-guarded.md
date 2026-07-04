# net-http AuthSource + Auth/SetHeaders Layers + Guarded Body (Slice 0, PR 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the first two construction-surface seams in `oath-adapter-net-http-api` — the `AuthSource` credential seam (trait + `NoAuth` + the `Auth` layer + the `SetHeaders` static-defaults layer) and the `Guarded<B>` permit-carrying response body — plus ADR-0034 recording the construction-surface decisions and the ADR-0030/0031 amendments.

**Architecture:** `AuthSource` is the dependency-inversion seam adapters implement; the `Auth` layer calls `authorize` on the final buffered request immediately before the leaf (innermost, inside `Retry` once `Retry` exists — so credentials are re-stamped per attempt). `SetHeaders` stamps static default headers *outside* `Auth`, so dynamic credentials win any key collision (last writer before the leaf). `Guarded<B>` wraps any response body and carries an `Option<async_lock::SemaphoreGuardArc>` concurrency permit, released at the **earlier of stream-end or drop** — the body-attach model (hyper's `Pooled` pattern), deliberately stronger than tower's response-future attach. `async-lock` (not `tokio::sync`) keeps `net-http-api`'s dependency graph runtime-free.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `http`/`http-body`/`bytes`/`pin-project-lite`/`thiserror` + **new: `async-lock` 3**. Still **no** `tokio`/`hyper`/`reqwest`/`serde` in `net-http-api`.

**Source spec:** [docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md](../specs/2026-06-30-net-http-construction-surface-design.md). This is **Slice 0, PR 3**; it builds on PR 2 (#60, merged) and follows the PR 3 roadmap in [2026-06-30-net-http-foundation.md](2026-06-30-net-http-foundation.md).

**Depends on PR 2 having merged** (it has — #60): PR 3 consumes `oath_adapter_net_http_api::{Service, HttpError}` and `oath_adapter_net_api::{ErrorKind, HasErrorKind}`.

## Decisions locked by this plan (the spec's open questions)

1. **ADR form (spec open-Q #1):** a standalone **ADR-0034** lands in this PR (Task 3.1), recording the construction-surface decisions and the two amendments — 0030/0031 stay append-only, each gaining only a one-line "amended by" pointer. The spec's 0032→0034 renumber edit (currently sitting uncommitted in the primary checkout) is applied in the worktree as part of Task 3.1; **after the PR merges, discard the primary checkout's copy** (`git checkout -- docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md`).
2. **Slice boundary (spec open-Q #2):** PR 3 is **one PR** — `AuthSource`/`NoAuth`/`Auth`/`SetHeaders` and `Guarded<B>` are all small additions to the same crate's contract surface; splitting would create two near-empty PRs.
3. **`MockTimer` home (spec open-Q #3):** already settled by PR 2 — it lives in `net-http-mock`. Nothing to do.
4. **Test doubles:** inline stubs in `net-http-api`'s unit tests (same as PR 2), **not** a dev-dep on `net-http-mock` — avoids the dev-dep cycle the foundation plan flagged. The mock-crate-driven, full-stack ordering tests arrive with `stack()` (Slice 2).
5. **Deferred, correctly absent here:** the `Retry` layer (so the literal "once per *attempt*" test lands with `Retry` in Slice 1 — this PR pins its foundation: a fresh stamp per `call`), `RateLimit` (the layer that will *construct* `Guarded` — PR 4/Slice 1), `HttpConfig.headers` wiring for `SetHeaders` (Slice 2's `stack()`).

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` (`unsafe_code = "deny"`; both crates `#![forbid(unsafe_code)]`). Body impls use `pin-project-lite`, never manual `unsafe`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result`. Test code is exempt for `unwrap`/`expect`/indexing only.
- **`just lint` runs clippy with `-D warnings` and promotes `pedantic`/`nursery`** — all code including tests must be pedantic-clean: `#[must_use]` where clippy asks, document all public items (`missing_docs`), `Debug` on all public types (`missing_debug_implementations`), no unreachable `pub`.
- **`net-http-api` charter:** free of any async runtime — no `tokio`/`hyper`/`reqwest`/`serde`. This PR adds exactly one dep: `async-lock` (runtime-agnostic; pulls `event-listener`/`event-listener-strategy`).
- **Deps** via `[workspace.dependencies]`. Add a dep in the task that first *uses* it (keeps `cargo-machete` green).
- **DoD per PR:** `just ci` green. Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree under `.claude/worktrees/<slug>` → one PR (`Closes #<issue>`).

---

## File Structure

- `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md` — **new.** The construction-surface ADR.
- `docs/adr/0030-http-transport-contract-wire-bytes-streaming-composition.md`, `docs/adr/0031-http-resilience-venue-pacing.md` — **modify.** One "amended by" pointer line each.
- `docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md` — **modify.** The 0032→0034 renumber (4 lines).
- `crates/adapter/net/http/api/src/auth.rs` — **new.** `AuthSource`, `NoAuth`, `Auth<S, A>`, `SetHeaders<S>`.
- `crates/adapter/net/http/api/src/body.rs` — **modify.** Add `Guarded<B>`.
- `crates/adapter/net/http/api/src/lib.rs` — **modify.** `mod auth` + re-exports.
- `crates/adapter/net/http/api/Cargo.toml` — **modify.** Add `async-lock`.
- `Cargo.toml` (workspace) — **modify.** Add `async-lock` to `[workspace.dependencies]`.
- `CHANGELOG.md` — **modify.**

Each task is one commit; the six tasks together are one PR/issue.

---

## Setup: issue + worktree

- [ ] **Create the issue and the isolated worktree**

```bash
gh issue create \
  --title "feat(net): AuthSource + Auth/SetHeaders layers + Guarded body (Slice 0, PR 3)" \
  --label enhancement \
  --body "Slice 0 PR 3 of the net-http construction surface (spec: docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md, plan: docs/superpowers/plans/2026-07-02-net-http-auth-guarded.md):

- \`AuthSource\` trait (RPITIT, \`Send\`-bounded) + \`NoAuth\` + the \`Auth\` layer (innermost credential stamping) + \`SetHeaders\` (static defaults outside Auth, dynamic wins)
- \`Guarded<B>\` permit-carrying response body over \`async-lock\` (released at the earlier of stream-end or drop; forwards \`is_end_stream\`/\`size_hint\`)
- ADR-0034 recording the construction-surface decisions and the ADR-0030/0031 amendments"
```

Note the issue number `#<N>` for the PR body.

```bash
git worktree add .claude/worktrees/net-http-auth-guarded -b feat/net-http-auth-guarded main
cd .claude/worktrees/net-http-auth-guarded
```

All subsequent tasks run inside this worktree.

---

## Task 3.1: ADR-0034 + amendment pointers + spec renumber

**Files:**
- Create: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `docs/adr/0030-http-transport-contract-wire-bytes-streaming-composition.md` (line 1 area), `docs/adr/0031-http-resilience-venue-pacing.md` (line 1 area), `docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md:462-468`

**Interfaces:**
- Consumes: the approved construction-surface spec; ADR-0030/0031 as landed.
- Produces: ADR-0034 — the document later tasks' doc-comments cite (`ADR-0031 §3 as amended by ADR-0034`).

- [ ] **Step 1: Write ADR-0034**

Create `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`:

````markdown
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
- **Errors are `HttpError`** via a new `Auth(String)` variant (→ `ErrorKind::Auth`) —
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

## Consequences

- `net-http-api` gains `async-lock` (+ `event-listener`) and stays runtime-free; the
  `HttpError::Auth` variant and the `Guarded<B>` type are part of the public contract.
- Adapters implement `AuthSource` once per venue; `NoAuth` ships in `net-http-api`.
- Implementation lands in slices: `AuthSource`/`Auth`/`SetHeaders`/`Guarded` first;
  `RateKey`/`RateLimitConfig`/`BuildError` next; the resilience layers and
  `stack()`/`build()` + the hyper leaf in later slices.
````

- [ ] **Step 2: Add the amendment pointers**

At the top of `docs/adr/0030-http-transport-contract-wire-bytes-streaming-composition.md`, insert directly under the `#` title line:

```markdown
> **Amended by [ADR-0034](0034-http-construction-surface-auth-guarded-boot-coverage.md):**
> the dependency charter is "free of any async *runtime*" (`async-lock` is allowed);
> `HttpError` gains an `Auth` variant; `ResponseBody<B>` must forward
> `is_end_stream`/`size_hint`; HTTP error statuses pass through as `Ok(Response)`.
```

At the top of `docs/adr/0031-http-resilience-venue-pacing.md`, insert directly under the `#` title line:

```markdown
> **Amended by [ADR-0034](0034-http-construction-surface-auth-guarded-boot-coverage.md):**
> §3's `Permit` enum is replaced by `Guarded<B>` carrying
> `Option<async_lock::SemaphoreGuardArc>`, released at the *earlier of* stream-end or
> drop; `RateLimitConfig<K>` must be total over `RateKey::all()` at construction.
```

- [ ] **Step 3: Apply the spec renumber**

In `docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md`, open question 1, replace:

```markdown
1. **ADR form** — fold the two amendments into 0030/0031 directly, or land a short
   ADR-0032 that records them and the construction-surface decisions? (The repo has
   landed 0029–0031 append-only; leaning ADR-0032.)
```

with:

```markdown
1. **ADR form** — fold the two amendments into 0030/0031 directly, or land a short
   **ADR-0034** that records them and the construction-surface decisions? (The repo has
   landed 0029–0031 append-only; leaning a standalone ADR. **Note:** 0032/0033 are taken by
   the WebSocket transport pair on branch `docs/adr-ws-transport-stack`, so this workstream's
   ADR is **0034**, not 0032.)
```

(This is the identical edit currently uncommitted in the primary checkout — discard that copy after merge.)

- [ ] **Step 4: Verify and commit**

Run: `just ci` (the docs gate: typos, markdown checks) — Expected: green.

```bash
git add docs/adr docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md
git commit -m "docs(adr): ADR-0034 — HTTP construction surface (AuthSource, Guarded, boot-time coverage)"
```

---

## Task 3.2: `AuthSource` trait + `NoAuth`

**Files:**
- Create: `crates/adapter/net/http/api/src/auth.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `crate::HttpError`, `bytes::Bytes`, `http::Request`.
- Produces: `oath_adapter_net_http_api::{AuthSource, NoAuth}` with `AuthSource::authorize(&self, req: &mut http::Request<Bytes>) -> impl Future<Output = Result<(), HttpError>> + Send`. Tasks 3.3–3.4 and the IBKR adapter build on exactly this signature.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/auth.rs` with the module doc and only the tests, and add `pub mod auth;` plus `pub use auth::{AuthSource, NoAuth};` to `lib.rs`:

```rust
//! The credential-stamping seam: `AuthSource`, `NoAuth`, and the `Auth` and
//! `SetHeaders` layers (ADR-0034).

#[cfg(test)]
mod tests {
    use super::{AuthSource, NoAuth};
    use bytes::Bytes;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    #[test]
    fn no_auth_is_ready_ok_and_leaves_request_untouched() {
        let mut req = http::Request::new(Bytes::new());
        {
            let fut = NoAuth.authorize(&mut req);
            let mut cx = Context::from_waker(Waker::noop());
            let mut fut = pin!(fut);
            // Immediately ready on first poll — no executor, no runtime.
            assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
        }
        assert!(req.headers().is_empty());
    }

    #[test]
    fn auth_source_futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut req = http::Request::new(Bytes::new());
        let fut = NoAuth.authorize(&mut req);
        assert_send(&fut);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find AuthSource`/`NoAuth` in this scope.

- [ ] **Step 3: Implement `AuthSource` + `NoAuth`**

Insert between the module doc and the tests in `auth.rs`:

```rust
use crate::HttpError;
use bytes::Bytes;
use std::future::Future;

/// The credential seam the adapter implements (ADR-0034 §1). The [`Auth`] layer
/// calls it innermost — inside `Retry`, once per attempt, against the final
/// buffered request — so per-attempt re-signing (fresh HMAC timestamp/nonce)
/// and current-token stamping are correct by construction.
pub trait AuthSource: Clone + Send + Sync {
    /// Stamp current credentials onto an outgoing request, immediately before
    /// send. Mutates in place (no clone — `Retry` already owns a per-attempt
    /// request). A failure (e.g. token refresh failed) is an
    /// [`HttpError::Auth`].
    fn authorize(
        &self,
        req: &mut http::Request<Bytes>,
    ) -> impl Future<Output = Result<(), HttpError>> + Send;
}

/// The no-op [`AuthSource`]: nothing to stamp. IBKR's local Client Portal
/// gateway holds the session cookie, so `authorize` is a ready `Ok(())`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAuth;

impl AuthSource for NoAuth {
    fn authorize(
        &self,
        _req: &mut http::Request<Bytes>,
    ) -> impl Future<Output = Result<(), HttpError>> + Send {
        std::future::ready(Ok(()))
    }
}
```

In `lib.rs`, extend the module doc list with the new module (final wording — the layers named here land in Tasks 3.3–3.4 of this same PR):

```rust
//! - [`auth`] — the `AuthSource` seam, `NoAuth`, and the `Auth`/`SetHeaders` layers
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api auth && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api
git commit -m "feat(net): AuthSource seam + NoAuth"
```

---

## Task 3.3: the `Auth<S, A>` layer

**Files:**
- Modify: `crates/adapter/net/http/api/src/auth.rs`, `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{Service, HttpError}`, Task 3.2's `AuthSource`.
- Produces: `oath_adapter_net_http_api::Auth`; `Auth::new(inner: S, auth: A) -> Auth<S, A>`; `impl Service<http::Request<Bytes>> for Auth<S, A>` with `Response = S::Response`, `Error = HttpError`. Slice 2's `stack()` composes it directly above the leaf.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `auth.rs` (extend the existing `use super::…` line to `use super::{Auth, AuthSource, NoAuth};` and add the new imports shown):

```rust
    use crate::{HttpError, Service};
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    /// Records every request it receives; the assertion surface for layer tests.
    #[derive(Clone, Default)]
    struct Recording {
        seen: Arc<Mutex<Vec<http::Request<Bytes>>>>,
    }

    impl Recording {
        fn seen(&self) -> Vec<http::Request<Bytes>> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Service<http::Request<Bytes>> for Recording {
        type Response = ();
        type Error = HttpError;

        fn call(
            &self,
            req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let seen = Arc::clone(&self.seen);
            async move {
                seen.lock().unwrap().push(req);
                Ok(())
            }
        }
    }

    /// Stamps a monotonically fresh `x-attempt` value on every authorize call.
    #[derive(Clone, Default)]
    struct Counting {
        n: Arc<AtomicU32>,
    }

    impl AuthSource for Counting {
        fn authorize(
            &self,
            req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
            req.headers_mut()
                .insert("x-attempt", http::HeaderValue::from(n));
            std::future::ready(Ok(()))
        }
    }

    /// Always fails — exercises the error short-circuit.
    #[derive(Clone)]
    struct Failing;

    impl AuthSource for Failing {
        fn authorize(
            &self,
            _req: &mut http::Request<Bytes>,
        ) -> impl Future<Output = Result<(), HttpError>> + Send {
            std::future::ready(Err(HttpError::auth("refresh failed")))
        }
    }

    #[tokio::test]
    async fn auth_stamps_the_request_the_inner_service_sees() {
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Counting::default());
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        let seen = leaf.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].headers()["x-attempt"], http::HeaderValue::from(1));
    }

    #[tokio::test]
    async fn auth_stamps_a_fresh_value_per_call() {
        // Each `call` is one attempt today; when `Retry` lands (Slice 1) it
        // re-invokes `call` per attempt, so this freshness IS the per-attempt
        // re-signing guarantee (ADR-0034 §1).
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Counting::default());
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        let seen = leaf.seen();
        assert_eq!(seen[0].headers()["x-attempt"], http::HeaderValue::from(1));
        assert_eq!(seen[1].headers()["x-attempt"], http::HeaderValue::from(2));
    }

    #[tokio::test]
    async fn authorize_error_short_circuits_and_classifies_as_auth() {
        let leaf = Recording::default();
        let client = Auth::new(leaf.clone(), Failing);
        let err = client
            .call(http::Request::new(Bytes::new()))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
        assert!(leaf.seen().is_empty(), "inner service must never be called");
    }

    #[test]
    fn auth_call_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let client = Auth::new(Recording::default(), NoAuth);
        let fut = client.call(http::Request::new(Bytes::new()));
        assert_send(&fut);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `just check`
Expected: FAIL — `cannot find Auth`.

- [ ] **Step 3: Implement `Auth`**

Add to `auth.rs` (below `NoAuth`, above the tests), and add `use crate::Service;` to the imports:

```rust
/// The credential-stamping layer: runs [`AuthSource::authorize`] on the final
/// request immediately before the inner service (ADR-0034 §1). Sits innermost
/// in the stack — inside `Retry` — so credentials are re-stamped per attempt.
#[derive(Debug, Clone)]
pub struct Auth<S, A> {
    inner: S,
    auth: A,
}

impl<S, A> Auth<S, A> {
    /// Wrap `inner`, stamping credentials from `auth` before every call.
    #[must_use]
    pub const fn new(inner: S, auth: A) -> Self {
        Self { inner, auth }
    }
}

impl<S, A> Service<http::Request<Bytes>> for Auth<S, A>
where
    S: Service<http::Request<Bytes>, Error = HttpError> + Sync,
    A: AuthSource,
{
    type Response = S::Response;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`,
    // which only the desugared form can promise (ADR-0029 §4).
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        mut req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<S::Response, HttpError>> + Send {
        async move {
            self.auth.authorize(&mut req).await?;
            self.inner.call(req).await
        }
    }
}
```

Extend the `lib.rs` re-export to `pub use auth::{Auth, AuthSource, NoAuth};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api auth && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api
git commit -m "feat(net): Auth layer — per-attempt credential stamping innermost"
```

---

## Task 3.4: the `SetHeaders<S>` layer

**Files:**
- Modify: `crates/adapter/net/http/api/src/auth.rs`, `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{Service, HttpError}`, `http::HeaderMap`.
- Produces: `oath_adapter_net_http_api::SetHeaders`; `SetHeaders::new(inner: S, headers: http::HeaderMap) -> SetHeaders<S>`. Slice 2's `stack()` folds `HttpConfig.headers` into this layer, just outside `Auth`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `auth.rs` (extend the `use super::…` line with `SetHeaders`):

```rust
    #[tokio::test]
    async fn set_headers_stamps_static_defaults() {
        let leaf = Recording::default();
        let mut headers = http::HeaderMap::new();
        headers.insert("user-agent", http::HeaderValue::from_static("oath"));
        let client = SetHeaders::new(leaf.clone(), headers);
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        assert_eq!(
            leaf.seen()[0].headers()["user-agent"],
            http::HeaderValue::from_static("oath")
        );
    }

    #[tokio::test]
    async fn dynamic_credentials_beat_static_headers_on_collision() {
        // The pinned composition: SetHeaders sits OUTSIDE Auth, so Auth is the
        // last writer before the leaf and dynamic credentials win (ADR-0034 §1).
        #[derive(Clone)]
        struct DynKey;
        impl AuthSource for DynKey {
            fn authorize(
                &self,
                req: &mut http::Request<Bytes>,
            ) -> impl Future<Output = Result<(), HttpError>> + Send {
                req.headers_mut()
                    .insert("x-api-key", http::HeaderValue::from_static("dynamic"));
                std::future::ready(Ok(()))
            }
        }

        let leaf = Recording::default();
        let mut headers = http::HeaderMap::new();
        headers.insert("x-api-key", http::HeaderValue::from_static("static"));
        let client = SetHeaders::new(Auth::new(leaf.clone(), DynKey), headers);
        client.call(http::Request::new(Bytes::new())).await.unwrap();
        assert_eq!(
            leaf.seen()[0].headers()["x-api-key"],
            http::HeaderValue::from_static("dynamic")
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `just check`
Expected: FAIL — `cannot find SetHeaders`.

- [ ] **Step 3: Implement `SetHeaders`**

Add to `auth.rs` below `Auth`:

```rust
/// Stamps static default headers (venue base headers, `User-Agent`, …) onto
/// every request. Sits **just outside** [`Auth`] in the canonical stack, so on
/// any key collision the dynamic credential wins — `Auth` is the last writer
/// before the leaf (ADR-0034 §1). Writes with insert (last-writer) semantics;
/// multi-valued defaults are not supported.
#[derive(Debug, Clone)]
pub struct SetHeaders<S> {
    inner: S,
    headers: http::HeaderMap,
}

impl<S> SetHeaders<S> {
    /// Wrap `inner`, stamping `headers` onto every request.
    #[must_use]
    pub const fn new(inner: S, headers: http::HeaderMap) -> Self {
        Self { inner, headers }
    }
}

impl<S> Service<http::Request<Bytes>> for SetHeaders<S>
where
    S: Service<http::Request<Bytes>, Error = HttpError> + Sync,
{
    type Response = S::Response;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`,
    // which only the desugared form can promise (ADR-0029 §4).
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        mut req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<S::Response, HttpError>> + Send {
        async move {
            for (name, value) in &self.headers {
                req.headers_mut().insert(name.clone(), value.clone());
            }
            self.inner.call(req).await
        }
    }
}
```

Extend the `lib.rs` re-export to `pub use auth::{Auth, AuthSource, NoAuth, SetHeaders};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api auth && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api
git commit -m "feat(net): SetHeaders layer — static defaults outside Auth, dynamic wins"
```

---

## Task 3.5: `Guarded<B>` — the permit-carrying body

**Files:**
- Modify: `crates/adapter/net/http/api/src/body.rs`, `crates/adapter/net/http/api/src/lib.rs`, `crates/adapter/net/http/api/Cargo.toml`, `Cargo.toml` (workspace)

**Interfaces:**
- Consumes: `http_body::{Body, Frame, SizeHint}`, `async_lock::SemaphoreGuardArc`, `crate::HttpError`.
- Produces: `oath_adapter_net_http_api::Guarded`; `Guarded::new(inner: B, permit: Option<async_lock::SemaphoreGuardArc>) -> Guarded<B>`; `impl http_body::Body for Guarded<B>` (`Data = Bytes`, `Error = HttpError`). Slice 1's `RateLimit` constructs it around every response body.

- [ ] **Step 1: Add the dependency**

In the workspace `Cargo.toml` `[workspace.dependencies]` (alphabetical position):

```toml
async-lock = "3"
```

In `crates/adapter/net/http/api/Cargo.toml` `[dependencies]`:

```toml
async-lock = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Add to the existing `tests` module in `body.rs` (extend its imports with the new items shown):

```rust
    use super::Guarded;
    use async_lock::Semaphore;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::task::Waker;

    /// Multi-frame inner body (inline double — no dev-dep on net-http-mock,
    /// same no-cycle choice as PR 2).
    struct Frames {
        frames: VecDeque<Bytes>,
    }

    impl Frames {
        fn new<const N: usize>(frames: [&'static [u8]; N]) -> Self {
            Self {
                frames: frames.iter().copied().map(Bytes::from_static).collect(),
            }
        }
    }

    impl Body for Frames {
        type Data = Bytes;
        type Error = HttpError;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            let this = self.get_mut();
            Poll::Ready(this.frames.pop_front().map(|data| Ok(Frame::data(data))))
        }

        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            let total: u64 = self.frames.iter().map(|f| f.len() as u64).sum();
            SizeHint::with_exact(total)
        }
    }

    #[test]
    fn guarded_forwards_size_hint_and_is_end_stream() {
        let reference = Frames::new([b"ab", b"cde"]);
        let ref_hint = reference.size_hint().exact();
        let wrapped = Guarded::new(Frames::new([b"ab", b"cde"]), None);
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently unbounded
        assert!(!wrapped.is_end_stream());
        let ended = Guarded::new(Frames::new([]), None);
        assert!(ended.is_end_stream()); // forwarded, not the `false` default
    }

    #[test]
    fn permit_releases_on_terminal_frame_before_drop() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let body = Guarded::new(Frames::new([b"a"]), Some(permit));
        assert!(sem.try_acquire_arc().is_none(), "permit held while unread");

        let mut cx = Context::from_waker(Waker::noop());
        let mut body = pin!(body);
        // Drain: data frame, then the terminal `None` (which must release).
        while let Poll::Ready(Some(res)) = body.as_mut().poll_frame(&mut cx) {
            res.expect("data frame");
        }
        // `body` is still alive here — the release was eager, not drop-driven.
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released at the terminal frame"
        );
    }

    #[test]
    fn permit_releases_on_early_drop() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        {
            let body = Guarded::new(Frames::new([b"a", b"b"]), Some(permit));
            let mut cx = Context::from_waker(Waker::noop());
            let mut body = pin!(body);
            // Read only the first frame — abort mid-stream.
            assert!(matches!(
                body.as_mut().poll_frame(&mut cx),
                Poll::Ready(Some(Ok(_)))
            ));
            assert!(sem.try_acquire_arc().is_none(), "still held mid-stream");
        }
        assert!(
            sem.try_acquire_arc().is_some(),
            "permit released on early drop"
        );
    }
```

Also extend the test module's `use` items with `use std::pin::pin;` if not present.

(All three tests are synchronous — no runtime, demonstrating the crate's runtime-neutrality.)

- [ ] **Step 3: Run them to verify they fail**

Run: `just check`
Expected: FAIL — `cannot find Guarded`.

- [ ] **Step 4: Implement `Guarded`**

Add to `body.rs` below `ResponseBody`'s impl (and add `use async_lock::SemaphoreGuardArc;`, `use std::fmt;`, and extend `use std::task::{Context, Poll};` with `ready`). Update the module doc to:

```rust
//! The canonical HTTP response body, the per-request buffer/stream directive,
//! and the permit-carrying `Guarded` wrapper.
```

```rust
pin_project_lite::pin_project! {
    /// Wraps a response body, carrying an optional concurrency permit released
    /// at the **earlier of stream-end or drop** (ADR-0031 §3 as amended by
    /// ADR-0034). `RateLimit` always returns `http::Response<Guarded<B>>`:
    /// `permit: None` for rate-scoped, unscoped, or buffered responses;
    /// `permit: Some(_)` only for a streaming concurrency-scoped response, so
    /// the permit rides the transfer instead of freeing at headers.
    pub struct Guarded<B> {
        #[pin]
        inner: B,
        permit: Option<SemaphoreGuardArc>,
    }
}

impl<B> Guarded<B> {
    /// Wrap `inner`, optionally carrying a concurrency `permit`.
    #[must_use]
    pub const fn new(inner: B, permit: Option<SemaphoreGuardArc>) -> Self {
        Self { inner, permit }
    }
}

impl<B: fmt::Debug> fmt::Debug for Guarded<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guarded")
            .field("inner", &self.inner)
            .field("permit_held", &self.permit.is_some())
            .finish()
    }
}

impl<B> Body for Guarded<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        if frame.is_none() {
            // Eager release at stream-end: a fully-read but still-held body
            // must not keep hold of one of the venue's concurrency slots.
            // Dropping the guard is synchronous and runtime-free.
            *this.permit = None;
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
```

In `lib.rs`, extend the body re-export to `pub use body::{BufferMode, Guarded, ResponseBody};`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api body && just lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/http/api Cargo.toml Cargo.lock
git commit -m "feat(net): Guarded body — concurrency permit released at stream-end or drop"
```

---

## Task 3.6: CHANGELOG, full gate, PR

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added`:

```markdown
- `oath-adapter-net-http-api` construction seams — `AuthSource` (per-attempt
  credential stamping) with `NoAuth`, the `Auth` layer (innermost, so `Retry`
  re-stamps per attempt) and `SetHeaders` (static defaults outside `Auth`,
  dynamic wins), and `Guarded` (response body carrying an optional `async-lock`
  concurrency permit, released at the earlier of stream-end or drop). ADR-0034
  records the construction-surface decisions and the ADR-0030/0031 amendments.
```

- [ ] **Step 2: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny — `async-lock`/`event-listener` must pass `cargo-deny` — typos).

- [ ] **Step 3: Commit, push, PR**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): net-http construction seams (Slice 0 PR 3)"
git push -u origin feat/net-http-auth-guarded
gh pr create \
  --title "feat(net): AuthSource + Auth/SetHeaders layers + Guarded body (Slice 0 PR 3)" \
  --body "Closes #<N>

Slice 0 PR 3 of the net-http construction surface (spec: docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md):

- **ADR-0034** — records the construction-surface decisions and amends ADR-0030/0031 append-only (runtime-free charter wording, \`HttpError::Auth\`, \`Guarded\` replacing the \`Permit\` enum, boot-time total pacing coverage).
- **\`AuthSource\`** (RPITIT, \`Send\`-bounded) + **\`NoAuth\`** + the **\`Auth\`** layer — credentials stamped on the final request innermost, so \`Retry\` (Slice 1) re-signs per attempt; authorize failures classify as \`ErrorKind::Auth\` and short-circuit.
- **\`SetHeaders\`** — static default headers just outside \`Auth\`; dynamic credentials win on collision.
- **\`Guarded<B>\`** — the concurrency permit rides the response body (\`async-lock\`, runtime-neutral), released at the earlier of stream-end or drop; forwards \`is_end_stream\`/\`size_hint\`.

Next: PR 4 (\`RateKey\` + \`RateLimitConfig<K>\` + boot-time coverage validation) closes Slice 0.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (PR 3 roadmap in the foundation plan + construction-surface spec):**
- `AuthSource` trait, exact spec signature (`&mut http::Request<Bytes>`, `impl Future + Send`, `HttpError` return) — Task 3.2. ✅
- `NoAuth` ready-`Ok(())`, polled without an executor — Task 3.2. ✅
- `Auth` layer: authorize-then-call, straight `?`, error → `ErrorKind::Auth`, inner never called on failure, fresh stamp per call (the per-attempt foundation; the literal retry test lands with `Retry`, Slice 1) — Task 3.3. ✅
- `SetHeaders` just outside `Auth`, dynamic-wins collision test — Task 3.4. ✅
- `Guarded<B>`: `pin-project-lite`, `Option<SemaphoreGuardArc>`, `ready!`-based eager release on terminal frame, forwards `is_end_stream`/`size_hint`, drop releases mid-stream — Task 3.5, tests are the spec's "Body transparency" + "Permit lifetime" items scoped to what exists pre-`RateLimit` (the (N+1)-th-concurrent-acquire and buffered-release-at-call-return tests need the `RateLimit` layer — Slice 1). ✅
- `async-lock` chosen over `tokio::sync`; dep added via `[workspace.dependencies]` in the task that first uses it — Task 3.5. ✅
- ADR reconciliation as a standalone ADR-0034 + append-only pointers + spec renumber — Task 3.1 (spec open-Q #1 decided). ✅
- CHANGELOG + `just ci` + one-issue-one-PR mechanics — Setup + Task 3.6. ✅
- Deferred (correctly absent): `Retry`/`RateLimit`/`CircuitBreaker`/`Tracing` (Slice 1), `RateKey`/`RateLimitConfig`/`BuildError`/coverage (PR 4), `stack()`/`build()`/hyper leaf (Slice 2), acquire-fairness test (needs `RateLimit`).

**Placeholder scan:** none — every code step carries the actual code, every run step the actual command.

**Type consistency:** `AuthSource::authorize` signature identical in Task 3.2's definition, Task 3.3's `Counting`/`Failing` impls, and Task 3.4's `DynKey`; `Auth::new(inner, auth)`/`SetHeaders::new(inner, headers)` argument order consistent between definitions and tests; `Guarded::new(inner, permit)` matches the `Interfaces` block and all three tests; `lib.rs` re-exports accumulate to `pub use auth::{Auth, AuthSource, NoAuth, SetHeaders};` and `pub use body::{BufferMode, Guarded, ResponseBody};`.

**Known risks to watch during impl:**
- `async-lock` API surface: `Semaphore::new`, `try_acquire_arc` (on `Arc<Semaphore>`), `SemaphoreGuardArc` — stable in async-lock 3.x; if a name differs, adjust at the failing-test step.
- The `#[allow(clippy::manual_async_fn)]` on `Auth::call`/`SetHeaders::call` mirrors the PR 2 test-code precedent (`client.rs`'s `Leaf`); if clippy does not fire there, drop the allow (an unused allow is itself a lint error under `-D warnings`).
- `Waker::noop()` (1.85) and `std::pin::pin!` are within MSRV 1.90.
- `cargo-deny`: `async-lock`/`event-listener`/`event-listener-strategy` are MIT/Apache-2.0 — should pass the license policy; if `deny` flags a duplicate-version, resolve in the Task 3.5 step before committing.
