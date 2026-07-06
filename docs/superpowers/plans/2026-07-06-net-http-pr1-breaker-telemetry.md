# net-http PR1 — Breaker + telemetry correctness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the confirmed breaker/telemetry defects (C1, M1, M2, L1) so a purely *local*
pacing rejection can never open the venue-wide circuit breaker, a cancelled non-probe call can
never reopen a concurrent Half-Open episode, breaker fast-rejects are labelled correctly, and
degenerate `Duration` configs saturate instead of panicking.

**Architecture:** Pure edits to `oath-adapter-net-http-api`. The pure `Breaker` state machine
and `classify`/`kind_label` free functions change; the async `CircuitBreaker` service shell
arms its cancellation `ProbeGuard` only for genuine Half-Open probes. One new tiny internal
`clock` helper for saturating deadline arithmetic. No public API change; no new dependency.

**Tech Stack:** Rust 2024 (MSRV 1.90), `just` (recipes pin CI flags), `oath-adapter-net-mock`
`MockTimer` for deterministic time in tests.

## Global Constraints (copied from the spec / CLAUDE.md)

- No `unsafe`; no `unwrap`/`expect`/indexing in non-test code (tests exempt). Model errors with
  `thiserror`.
- `missing_docs` warned — document new public items (none expected here; `pub(crate)` items
  should still carry a doc line).
- Conventional Commits (`fix(net): …`); clippy `all` is **deny-level**; edition 2024, MSRV 1.90.
- **Definition of done: `just ci` passes** (fmt, lint, test, doc, deny, typos). Also run
  `just doc` — broken intra-doc links pass check/lint/test but fail `just doc`.
- Update `CHANGELOG.md` `[Unreleased]` in this PR.
- Work in the `net-http-tier1` worktree on branch `fix/net-http-breaker-telemetry`; PR
  `Closes #<issue>`.

**Files touched (all under `crates/adapter/net/http/api/`):**
- `src/circuit_breaker.rs` — `classify` (C1), `Admit`/`Breaker::admit` + `CircuitBreaker::call`
  (M1), saturating deadlines (L1), tests.
- `src/trace.rs` — `kind_label` `CircuitOpen` arm (M2), test.
- `src/clock.rs` — **create**: `pub(crate) fn deadline(now, dur)` saturating helper (L1).
- `src/rate_limit.rs` — use `clock::deadline` for `now + max_wait` (L1).
- `src/lib.rs` — `mod clock;`.
- `../../../../CHANGELOG.md` — `[Unreleased]` entry.
- `docs/adr/0031-http-resilience-venue-pacing.md` — append-only §5 clarification.

---

### Task 1: C1 — a local `Throttled` error must not trip the breaker

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (`classify`, ~L72-97; doc
  comments ~L4-8, L53-58; tests)
- Test: same file (`classify_tests`, `service_tests`)

**Interfaces:**
- Consumes: `HttpError`, `ErrorKind`, `Class` (unchanged signatures).
- Produces: `classify` behaviour — error-side `ErrorKind::Throttled` → `Class::Ignored`;
  `Ok(status==429)` → `Class::TripNow` (unchanged).

- [ ] **Step 1: Update the pure-classify test to the correct expectation (red).**
  In `mod classify_tests`, replace `throttle_and_429_trip_now` with:

```rust
    #[test]
    fn only_a_429_response_trips_now_not_a_local_throttled_error() {
        // A `Throttled` *error* is produced only locally by RateLimit (max_wait /
        // fail-closed reject) — the request never reached the host, so it carries
        // no host-health signal and must be Ignored, never TripNow (ADR-0031 §5).
        assert_eq!(classify::<()>(&Err(HttpError::Throttled)), Class::Ignored);
        // A real venue 429 *response* still trips.
        assert_eq!(classify(&ok(429)), Class::TripNow);
    }
```

- [ ] **Step 2: Add a full-stack regression test (red).**
  In `mod service_tests`, add a test proving repeated local throttles keep the breaker Closed:

```rust
    #[tokio::test]
    async fn repeated_local_throttle_never_opens_the_breaker() {
        // failure_threshold is 3; fire 5 local (absent-RateScope) Throttleds — none
        // reached the leaf. The breaker must stay Closed: a well-formed request then
        // still reaches the leaf instead of being fast-rejected as CircuitOpen.
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(
            leaf.clone(),
            http_cfg(1, Duration::from_secs(30), Duration::ZERO),
            timer,
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");
        for _ in 0..5 {
            let bare = http::Request::builder()
                .method("GET")
                .uri("/x")
                .body(Bytes::new())
                .unwrap(); // no RateScope → RateLimit fails closed with Throttled
            assert!(matches!(svc.call(bare).await, Err(HttpError::Throttled)));
        }
        let resp = svc
            .call(req(Scope::Global, None))
            .await
            .expect("breaker stayed Closed after local throttles");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 1, "only the well-formed request reached the leaf");
    }
```

  > Note: this test lives in `stack.rs`'s `service_tests`, not `circuit_breaker.rs` — it needs
  > the assembled `stack()`. Put Step 2 in `crates/adapter/net/http/api/src/stack.rs` `mod
  > tests` (reuse its `ScriptLeaf`, `http_cfg`, `rate_cfg`, `req`, `stack`, `Scope`, `NoAuth`).

- [ ] **Step 3: Run both tests to confirm they fail.**
  Run: `cd .claude/worktrees/net-http-tier1 && cargo test -p oath-adapter-net-http-api -- only_a_429_response_trips_now repeated_local_throttle`
  Expected: FAIL — `classify(Err(Throttled))` is `TripNow` (want `Ignored`); the stack test gets
  `CircuitOpen` on the well-formed request (breaker opened) and `leaf.calls() == 0`.

- [ ] **Step 4: Fix `classify` (green).**
  In `circuit_breaker.rs`, delete the `ErrorKind::Throttled => Class::TripNow,` arm so it falls
  into `_ => Class::Ignored`:

```rust
        Err(e) => match e.kind() {
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Server => Class::Failure,
            // A `Throttled` *error* is a purely LOCAL pacing decision (RateLimit's
            // max_wait breach / fail-closed reject) — the request never reached the
            // host, so it carries zero host-health signal and must NOT trip the
            // breaker. Only a real venue 429 *response* (the Ok-side arm) trips.
            // (ADR-0031 §5, clarified.) Auth/Client/Unknown/CircuitOpen and any
            // future kind are likewise Ignored.
            _ => Class::Ignored,
        },
```

- [ ] **Step 5: Update the module + `Class::TripNow` doc comments.**
  Module doc (~L7-8): change "or **immediately** on a `Throttled`/429" to "or **immediately** on
  a venue **429 response** (`Throttled` *errors* are local pacing decisions and are ignored)".
  `Class::TripNow` doc (~L54): change "A throttle/429 — trips the circuit **immediately**" to
  "A venue **429 response** — trips the circuit **immediately** on the long cooldown. (A
  `Throttled` *error* is local and does not reach here.)". `classify` doc (~L63-71): change
  "`Throttled`/429 is `TripNow`" to "a venue **429 response** is `TripNow`; a `Throttled`
  *error* is a local decision and is `Ignored`".

- [ ] **Step 6: Run the tests to confirm they pass.**
  Run: `cargo test -p oath-adapter-net-http-api -- only_a_429_response_trips_now repeated_local_throttle`
  Expected: PASS. Also re-run the whole crate: `cargo test -p oath-adapter-net-http-api`
  Expected: PASS (the old `a_single_429_trips_immediately_on_the_long_cooldown` still passes —
  it uses `Step::Status(429)`, the Ok-side arm, which is unchanged).

- [ ] **Step 7: Commit.**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/stack.rs
git commit -m "fix(net): local Throttled error no longer trips the circuit breaker (C1)"
```

---

### Task 2: M1 — arm the cancellation guard only for Half-Open probes

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (`Admit` enum ~L116-122;
  `Breaker::admit` ~L147-173; `CircuitBreaker::call` ~L385-421; breaker tests)

**Interfaces:**
- Produces: `enum Admit { Pass, Reject, Probe }` — `Pass` = normal Closed pass; `Probe` =
  Half-Open probe admitted (arm the guard); `Reject` = fast-reject. `Breaker::admit(&mut self,
  now) -> Admit` returns `Probe` for every Half-Open admission (the Open→Half-Open transition
  admit and subsequent in-episode admits), `Pass` in Closed, `Reject` when Open-and-cooling or
  the probe budget is spent.

- [ ] **Step 1: Add the pure-breaker test distinguishing probe from pass (red).**
  In `mod breaker_tests`:

```rust
    #[test]
    fn admit_distinguishes_a_probe_from_a_normal_pass() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1));
        assert_eq!(b.admit(now), Admit::Pass, "closed → normal pass, not a probe");
        b.record(Class::Failure, now); // → Open
        let after = now + Duration::from_secs(30);
        assert_eq!(b.admit(after), Admit::Probe, "half-open admission is a probe");
    }
```

- [ ] **Step 2: Add a service test — a cancelled *non-probe* call must not reopen a concurrent Half-Open (red).**
  In `mod service_tests`, add a leaf whose first call hangs (the Closed-era call we cancel) and
  whose later calls fail-once-then-hang, plus the test. (Reuses `MockTimer`, `bare_req`, `cfg`.)

```rust
    // Call 0 hangs forever (the Closed-era call we cancel). Call 1 fails (to trip
    // the breaker). Call 2+ hang (the live Half-Open probe).
    #[derive(Clone)]
    struct HangFailHangLeaf {
        calls: Arc<AtomicUsize>,
    }
    impl HangFailHangLeaf {
        fn new() -> Self {
            Self { calls: Arc::new(AtomicUsize::new(0)) }
        }
    }
    impl Service<http::Request<Bytes>> for HangFailHangLeaf {
        type Response = http::Response<()>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            async move {
                if i == 1 {
                    Err(err_of(ErrorKind::Connection))
                } else {
                    std::future::pending::<Result<http::Response<()>, HttpError>>().await
                }
            }
        }
    }

    #[tokio::test]
    async fn cancelling_a_non_probe_call_does_not_reopen_a_concurrent_half_open() {
        let timer = MockTimer::new();
        let leaf = HangFailHangLeaf::new();
        // threshold 1 → one failure opens; 1 probe per episode.
        let svc =
            CircuitBreakerLayer::new(cfg(1, secs(30), secs(900), 1), timer.clone()).layer(leaf);

        // 1. Admit a Closed-era call (call 0). It hangs; its future is parked.
        let mut closed_call = Box::pin(svc.call(bare_req()));
        assert!(futures_util::poll!(closed_call.as_mut()).is_pending());

        // 2. A second call fails → breaker Open (call 1).
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::Connection(_)
        ));
        // 3. Cooldown elapses; admit the real probe (call 2). It hangs (parked).
        timer.advance(secs(30));
        let mut probe = Box::pin(svc.call(bare_req()));
        assert!(futures_util::poll!(probe.as_mut()).is_pending());

        // 4. Cancel the Closed-era call. With probe-only guarding it was never a
        //    probe, so its Drop must NOT reopen the live Half-Open episode.
        drop(closed_call);

        // 5. The probe budget is spent (1 probe in flight), so a further call is
        //    rejected — the episode is intact, not reopened-then-fresh-cooldown.
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
    }
```

- [ ] **Step 3: Run to confirm failure.**
  Run: `cargo test -p oath-adapter-net-http-api -- admit_distinguishes cancelling_a_non_probe`
  Expected: FAIL — `Admit::Probe` variant does not exist (compile error), which also fails the
  service test's premise. (Compile error counts as red.)

- [ ] **Step 4: Add the `Probe` variant and make `admit` return it (green, part 1).**
  Change the `Admit` enum and its docs:

```rust
/// The admission verdict for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Admit a normal (Closed-state) call to the inner stack.
    Pass,
    /// Admit a **Half-Open probe** — the call whose outcome resolves the episode.
    /// The service arms its cancellation guard only for this verdict.
    Probe,
    /// Reject the call fast with `CircuitOpen` — the inner stack is not touched.
    Reject,
}
```

  In `Breaker::admit`, change the two Half-Open admission returns from `Admit::Pass` to
  `Admit::Probe` (the Open→Half-Open transition branch and the `probes_left > 0` branch); leave
  the `Closed` branch returning `Admit::Pass` and the reject branches returning `Admit::Reject`.

- [ ] **Step 5: Arm the guard only for probes in `CircuitBreaker::call` (green, part 2).**
  Replace the admit/guard block:

```rust
            let admit = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                breaker.admit(now)
            };
            let is_probe = match admit {
                Admit::Reject => return Err(HttpError::CircuitOpen),
                Admit::Probe => true,
                Admit::Pass => false,
            };
            // Arm the drop-guard ONLY for a genuine Half-Open probe: only a probe's
            // abandonment should reopen the episode. A cancelled Closed-state call
            // carries no probe semantics and must not touch a concurrent Half-Open.
            let mut guard = is_probe.then(|| ProbeGuard::arm(&self.breaker, &self.timer));
            let outcome = self.inner.call(req).await; // NO lock held across the await
            if let Some(g) = guard.as_mut() {
                g.disarm(); // completed normally — record the true outcome below
            }
```

  (The `record` block below is unchanged.)

- [ ] **Step 6: Update the existing breaker tests for the `Probe` verdict.**
  In `mod breaker_tests`, change the probe-admit assertions from `Admit::Pass` to `Admit::Probe`
  (the lines whose message mentions "probe"/"first probe"/"probe 1"/"probe 2"/"self-healed"):
  `open_rejects_until_cooldown_then_admits_one_probe`, `half_open_probe_success_closes` (first
  admit only — the post-success one is a Closed `Pass`), `half_open_probe_ignored_also_closes`
  (first admit only), `half_open_probe_failure_reopens` (both probe admits),
  `multi_probe_half_open_requires_all_to_close` (probe 1 & 2 only; the final "closed" admit stays
  `Pass`), `abandoned_probe_reopens_half_open` (both probe admits), `throttle_trips_immediately_…`
  (the "first probe admitted" line), `abandoned_probe_is_a_noop_in_open` (the "original cooldown
  still elapses" line is a probe admit → `Probe`). Closed-state passes stay `Admit::Pass`.

- [ ] **Step 7: Run to confirm all pass.**
  Run: `cargo test -p oath-adapter-net-http-api`
  Expected: PASS. Confirm `a_cancelled_half_open_probe_reopens_instead_of_wedging` (the existing
  probe-cancel test) still passes — a cancelled *probe* still reopens; only *non-probe*
  cancellation changed.

- [ ] **Step 8: Commit.**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs
git commit -m "fix(net): arm CircuitBreaker probe guard only for Half-Open probes (M1)"
```

---

### Task 3: M2 — label `CircuitOpen` in telemetry

**Files:**
- Modify: `crates/adapter/net/http/api/src/trace.rs` (`kind_label` ~L30-40; test)

- [ ] **Step 1: Add the test (red).** In `trace.rs` tests (add a `#[test]` if none targets
  `kind_label`; `kind_label` is `pub(crate)` so an in-file test can call it):

```rust
    #[test]
    fn circuit_open_has_its_own_label() {
        assert_eq!(super::kind_label(ErrorKind::CircuitOpen), "circuit_open");
    }
```

  (Ensure `use oath_adapter_net_api::ErrorKind;` is in scope for the test module.)

- [ ] **Step 2: Run to confirm failure.**
  Run: `cargo test -p oath-adapter-net-http-api circuit_open_has_its_own_label`
  Expected: FAIL — `kind_label(CircuitOpen)` returns `"unknown"` via the `_` arm.

- [ ] **Step 3: Add the arm (green).** In `kind_label`, add before the `_` arm:

```rust
        ErrorKind::CircuitOpen => "circuit_open",
```

- [ ] **Step 4: Run to confirm pass.**
  Run: `cargo test -p oath-adapter-net-http-api circuit_open_has_its_own_label`
  Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/adapter/net/http/api/src/trace.rs
git commit -m "fix(net): add circuit_open telemetry label (M2)"
```

---

### Task 4: L1 — saturating deadline arithmetic

**Files:**
- Create: `crates/adapter/net/http/api/src/clock.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs` (`mod clock;`),
  `src/circuit_breaker.rs` (5 `now + cooldown/throttle_cooldown` sites), `src/rate_limit.rs`
  (`now + max_wait`, ~L232)

**Interfaces:**
- Produces: `pub(crate) fn crate::clock::deadline(now: Instant, dur: Duration) -> Instant` —
  `now + dur`, saturating to a far-future instant on overflow instead of panicking.

- [ ] **Step 1: Create `clock.rs` with the helper and its test.**

```rust
//! Saturating deadline arithmetic — `Instant + Duration` that never panics on a
//! degenerate (e.g. `Duration::MAX` "no limit" sentinel) config (L1).

use std::time::{Duration, Instant};

/// `now + dur`, saturating to a far-future instant instead of panicking when the
/// sum overflows the platform `Instant`. Overflow means "effectively never", so
/// saturating forward (not to `now`) keeps the safe direction for cooldown and
/// permit-wait deadlines.
pub(crate) fn deadline(now: Instant, dur: Duration) -> Instant {
    now.checked_add(dur).unwrap_or_else(|| {
        // The sum overflowed; ~136 years is a valid far-future stand-in on every
        // real platform. If even that overflows, fall back to `now`.
        now.checked_add(Duration::from_secs(u64::from(u32::MAX)))
            .unwrap_or(now)
    })
}

#[cfg(test)]
mod tests {
    use super::deadline;
    use std::time::{Duration, Instant};

    #[test]
    fn normal_add_matches_plain_addition() {
        let now = Instant::now();
        assert_eq!(deadline(now, Duration::from_secs(30)), now + Duration::from_secs(30));
    }

    #[test]
    fn overflow_saturates_forward_instead_of_panicking() {
        let now = Instant::now();
        // Duration::MAX would panic under `now + dur`; deadline must not panic and
        // must land in the future.
        assert!(deadline(now, Duration::MAX) > now);
    }
}
```

- [ ] **Step 2: Register the module.** In `src/lib.rs`, add `mod clock;` (private — not `pub`)
  alongside the other `mod` declarations (before the `pub mod` block is fine; keep it
  non-public).

- [ ] **Step 3: Run the helper test to confirm it passes.**
  Run: `cargo test -p oath-adapter-net-http-api clock::`
  Expected: PASS.

- [ ] **Step 4: Use `deadline` at the breaker sites.**
  In `circuit_breaker.rs`, add `use crate::clock::deadline;` and replace each
  `now + self.cfg.cooldown` → `deadline(now, self.cfg.cooldown)` and each
  `now + self.cfg.throttle_cooldown` → `deadline(now, self.cfg.throttle_cooldown)` in
  `Breaker::record` (Closed `Failure`/`TripNow`, HalfOpen `Failure`/`TripNow`) and
  `Breaker::on_abandoned_probe` (5 sites total).

- [ ] **Step 5: Use `deadline` at the rate-limit site.**
  In `rate_limit.rs`, add `use crate::clock::deadline;` and replace
  `let deadline = self.timer.now() + self.max_wait;` (~L232) with
  `let deadline = deadline(self.timer.now(), self.max_wait);`. (Rename the local if it shadows
  the fn — e.g. keep the local named `deadline` but call `crate::clock::deadline(...)` fully
  qualified to avoid the name clash, or name the local `acquire_deadline`.)

- [ ] **Step 6: Run the full crate tests + clippy.**
  Run: `cargo test -p oath-adapter-net-http-api && cargo clippy -p oath-adapter-net-http-api --all-targets`
  Expected: PASS, no clippy warnings.

- [ ] **Step 7: Commit.**

```bash
git add crates/adapter/net/http/api/src/clock.rs crates/adapter/net/http/api/src/lib.rs \
        crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "fix(net): saturating deadline arithmetic for degenerate Duration configs (L1)"
```

---

### Task 5: ADR clarification + CHANGELOG + full CI gate

**Files:**
- Modify: `docs/adr/0031-http-resilience-venue-pacing.md` (append-only clarification under §5)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Append an ADR-0031 §5 clarification.**
  Add, in the same append-only amendment style ADR-0034 uses, a note that error-side
  `HttpError::Throttled` is a *local* pacing decision (request never sent) classified `Ignored`,
  while only a venue **429 response** trips the breaker — clarifying §5's "or immediately on a
  `Throttled`/429" which conflated the two. Reference this PR / finding C1.

- [ ] **Step 2: Add the CHANGELOG entry.** Under `[Unreleased]`:

```markdown
### Fixed
- **net-http:** a local pacing rejection (`HttpError::Throttled`) no longer trips the
  circuit breaker into the throttle-cooldown penalty box; only a venue `429` response does
  (C1). The Half-Open cancellation guard is armed only for genuine probes (M1). `CircuitOpen`
  now has its own `circuit_open` telemetry label (M2). Cooldown/permit-wait deadline
  arithmetic saturates instead of panicking on degenerate `Duration` configs (L1).
```

- [ ] **Step 3: Run the full local CI gate.**
  Run: `just ci && just doc`
  Expected: all green (fmt, lint, test, doc, deny, typos). Fix anything that fails; do not
  bypass hooks.

- [ ] **Step 4: Commit the docs.**

```bash
git add docs/adr/0031-http-resilience-venue-pacing.md CHANGELOG.md
git commit -m "docs(net): clarify ADR-0031 §5 (Throttled-error vs 429-response) + changelog"
```

- [ ] **Step 5: Push and open the PR.**

```bash
git push -u origin fix/net-http-breaker-telemetry
gh pr create --title "fix(net): breaker + telemetry correctness (C1, M1, M2, L1)" \
  --body "Closes #<issue>. See docs/superpowers/plans/2026-07-06-net-http-pr1-breaker-telemetry.md."
```

---

## Self-review

- **Spec coverage:** PR1 spec items — C1 (Task 1), M1 (Task 2), M2 (Task 3), L1 (Task 4),
  ADR-0031 §5 clarification (Task 5). All covered.
- **Placeholder scan:** none — every code step shows the code; `#<issue>` is filled at PR time.
- **Type consistency:** `Admit { Pass, Probe, Reject }` used consistently in `admit`, `call`, and
  the updated tests; `clock::deadline(now, dur) -> Instant` used identically at all sites.
- **Risk:** Task 2's service test uses `Box::pin` + `futures_util::poll!` like the existing
  `a_cancelled_half_open_probe_reopens_instead_of_wedging`; `futures_util` is already a dev-usable
  dep. If the concurrent-race test proves flaky under MockTimer, fall back to asserting the
  behaviour at the `admit`/guard boundary (pure test) and keep the probe-only wiring covered by
  the existing probe-cancel regression.

## Later PRs

PR2–PR9 are planned just-in-time (each gets its own `docs/superpowers/plans/…` doc before
execution), because PR5's breaking `RateScope`/`ResponseBody` changes and PR7a's metrics-library
decision affect the shape of PR6/PR8. Acceptance criteria for each are fixed in the
[spec](../specs/2026-07-06-net-http-tier1-remediation.md).
