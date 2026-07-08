# net-http Test-Debt + Docs (Tier-1 PR8 / issue #101) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the resilience-layer test gaps (M10), pin the two reasoned-not-observed findings, add doctests/examples/README for the per-request extension protocol, fix stale rustdoc + tautological tests, and record the loom-defer decision — the final Tier-1 remediation PR for the net-http stack.

**Architecture:** Pure **test + docs** work. Every production behavior these tests assert was already fixed and shipped in #104–#113; #101 adds the *regression guards*. Each test must be constructed so it would **fail if the specific bug were reintroduced** — the plan states that "guard mutation" for each. New tests are added to the existing inline `#[cfg(test)]` modules (the repo uses no `tests/` dirs); the real-leaf integration tests live in `net-http-hyper` (it already prod-depends on `net-http-api::stack`, so no cycle).

**Tech Stack:** Rust 2024, tokio (dev-only), `oath-adapter-net-mock::MockTimer` (virtual clock), `hyper`/`hyper-util` loopback servers, `metrics-util` debugging recorder, `http-body-util`. No new production dependencies.

## Global Constraints

- **Edition 2024, MSRV 1.90.** Validate with `just msrv` (final task only).
- **No `unwrap`/`expect`/indexing/`unsafe` in non-test code** (clippy `all` deny-level). **Test code is exempt** — new test code may use `unwrap`/`expect` freely, matching the existing test modules.
- **`missing_docs` is warn-level** — every new `pub` item (examples don't count) needs a `///` doc.
- **Conventional Commits**, enforced by the `commit-msg` hook. Use `test(net): …`, `docs(net): …`, `chore(net): …`.
- **Definition of done = `just ci` passes** (fmt, lint, test, doc, deny, typos). Doctests run under `just test`; broken intra-doc links only surface under **`just doc`** — run both.
- **One PR** (`Closes #101`), branched `test/net-http-test-debt` off `main`, in the shared Tier-1 worktree.
- **CHANGELOG:** add one `[Unreleased]` entry (final task).

---

## Setup (once, before Task 1)

The shared Tier-1 worktree `.claude/worktrees/net-http-tier1` already exists but is **detached at `5d52bdf`** (stale, pre-#107). Reset it to current `main` and branch:

```bash
git -C /workspaces/oath fetch origin
git -C /workspaces/oath/.claude/worktrees/net-http-tier1 reset --hard origin/main
git -C /workspaces/oath/.claude/worktrees/net-http-tier1 switch -c test/net-http-test-debt
```

All file paths below are relative to the repo root; edit them **inside the worktree** (`.claude/worktrees/net-http-tier1/…`), never the primary checkout. Run every `just`/`cargo` command with that worktree as the working directory.

**Baseline check** — confirm green before adding anything:

```bash
just check && just test
```
Expected: PASS (HEAD is post-#113, all green).

---

## Group A — Resilience test gaps (M10 + burst over-admission)

### Task 1: RateLimit proactive wait+refill park loop (M10-wait-refill)

**Files:**
- Test: `crates/adapter/net/http/api/src/rate_limit.rs` (append inside `mod tests`, near line 717)

**Interfaces (all existing, verified):**
- Consumes: `layer(timer: MockTimer, max_wait: Duration) -> RateLimitLayer<Key, MockTimer>`; `Leaf::ok(b"..")`; `req(RateScope::Local(Key::Snapshot))`; `MockTimer::{new, clone, advance}`. `Key::Snapshot` = TokenBucket 2/s burst 2.
- The park loop under test: `acquire_rate` (rate_limit.rs:261-293) — with `max_wait > 0`, a drained bucket **sleeps** `timer.sleep(wait)` then re-locks/refills/re-checks. Every current rate test uses `max_wait = 0`, so this loop is dead in tests.

- [ ] **Step 1: Write the test** (uses the spawn-then-advance pattern from `concurrency_waits_within_max_wait_then_succeeds`, rate_limit.rs:646)

```rust
    #[tokio::test]
    async fn rate_park_loop_sleeps_then_refills_and_succeeds() {
        // Snapshot = 2/s burst 2. Drain both tokens, then a third request with a
        // GENEROUS max_wait must PARK in acquire_rate (timer.sleep), not throttle.
        // Advancing the clock past the refill window wakes it and it succeeds — the
        // proactive wait+refill path that every max_wait=0 test skips.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(5)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1st drains a token");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2nd drains the last token");

        // Third: bucket empty, but max_wait = 5s > the 500ms refill interval → it must
        // park on timer.sleep rather than return Throttled. Spawn it, let it register
        // the sleep, then advance the clock to refill one token and wake it.
        let svc2 = svc.clone();
        let waiter =
            tokio::spawn(async move { svc2.call(req(RateScope::Local(Key::Snapshot))).await });
        tokio::task::yield_now().await; // task locks the bucket, sees empty, arms timer.sleep
        timer.advance(Duration::from_millis(500)); // 2 tokens/sec → +1 token, wakes the sleeper
        waiter
            .await
            .unwrap()
            .expect("parked request refilled within max_wait and succeeded");
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test -p oath-adapter-net-http-api rate_park_loop_sleeps_then_refills_and_succeeds -- --nocapture`
Expected: **PASS** (the park loop already works). **Guard mutation:** if `acquire_rate` ever returns `Throttled` before sleeping when `now + wait <= deadline` (i.e. the park branch regresses to fail-fast), the spawned task resolves to `Err(Throttled)` and `waiter.await.unwrap().expect(...)` panics. If the test *hangs* instead of passing, the `advance` isn't waking the sleeper — a real regression in the refill/wake path.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "test(net): exercise the RateLimit wait+refill park loop with max_wait>0 (M10)"
```

---

### Task 2: RateLimit tight refill-rate assertion (M10-refill-rate)

**Files:**
- Test: `crates/adapter/net/http/api/src/rate_limit.rs` (append inside `mod tests`)

**Interfaces:** same helpers as Task 1. Existing refill tests only assert a *lower bound* (≥1 token after a full period), so a **2× over-refill** bug passes. This test drains, advances an **exact** window, admits exactly the expected count, then asserts the very next call **throttles** — pinning the rate from **both** sides.

- [ ] **Step 1: Write the test**

```rust
    #[tokio::test]
    async fn refill_rate_is_exact_not_just_a_lower_bound() {
        // Snapshot = 2 tokens/sec, burst 2. Drain both, then advance ONLY 500ms so the
        // correct refill is exactly 1 token (0.5s × 2/s = 1) — strictly BELOW burst, so
        // the burst cap can't mask an inflated rate. Admit exactly 1; the next throttles.
        // A 2x-over-refill bug credits 2 tokens in 500ms → admits a 2nd → this fails,
        // WITHOUT needing the burst cap to also be broken.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("1");
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("2");
        assert!(matches!(
            svc.call(req(RateScope::Local(Key::Snapshot)))
                .await
                .unwrap_err(),
            HttpError::Throttled
        ));
        timer.advance(Duration::from_millis(500)); // exactly 1 token, < burst 2
        svc.call(req(RateScope::Local(Key::Snapshot)))
            .await
            .expect("exactly 1 token refilled");
        assert!(
            matches!(
                svc.call(req(RateScope::Local(Key::Snapshot)))
                    .await
                    .unwrap_err(),
                HttpError::Throttled
            ),
            "only 1 token accrued in 500ms (rate=2/s) — a 2nd admit would mean an over-refill"
        );
    }

    #[tokio::test]
    async fn partial_period_does_not_over_refill() {
        // 2 tokens/sec: after only 250ms (< the 500ms/token interval) NO token has
        // accrued, so a drained bucket still throttles. Catches an off-by-one-fast
        // refill that credits a fractional token as whole.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Local(Key::Snapshot))).await.expect("1");
        svc.call(req(RateScope::Local(Key::Snapshot))).await.expect("2");
        timer.advance(Duration::from_millis(250)); // < 500ms → no whole token yet
        assert!(
            matches!(
                svc.call(req(RateScope::Local(Key::Snapshot))).await.unwrap_err(),
                HttpError::Throttled
            ),
            "a quarter-period must not refill a whole token"
        );
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p oath-adapter-net-http-api refill_rate_is_exact_not_just_a_lower_bound partial_period_does_not_over_refill`
Expected: **PASS**. **Guard mutation:** doubling the refill rate (or dropping the burst cap) makes `refill_rate_is_exact_…` admit a 3rd request → the final `matches!(Throttled)` assert fails; crediting a fractional token as whole makes `partial_period_…` admit → fails.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "test(net): pin token-bucket refill rate from both sides (M10)"
```

---

### Task 3: RateScope::Both driven through acquire() (M10-both-order)

**Files:**
- Test: `crates/adapter/net/http/api/src/rate_limit.rs` (append inside `mod tests`)

**Interfaces:** `config()` gives global 10/s and `Key::Snapshot` local 2/s. A `RateScope::Both(Key::Snapshot)` spends **global then local** in one `acquire()`. No test currently drives `Both` through `acquire()`.

- [ ] **Step 1: Write the test**

```rust
    #[tokio::test]
    async fn both_scope_spends_global_and_local_in_one_acquire() {
        // Both(Snapshot) must acquire the global bucket AND the Snapshot local bucket.
        // Snapshot burst = 2 is the tighter of the two (global burst = 10), so the 3rd
        // Both request throttles on the drained LOCAL bucket — proving both buckets are
        // consulted (a Both that only spent global would admit up to 10).
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        svc.call(req(RateScope::Both(Key::Snapshot))).await.expect("1 (global+local)");
        svc.call(req(RateScope::Both(Key::Snapshot))).await.expect("2 (global+local)");
        assert!(
            matches!(
                svc.call(req(RateScope::Both(Key::Snapshot))).await.unwrap_err(),
                HttpError::Throttled
            ),
            "3rd Both throttles on the drained LOCAL bucket → both buckets were spent"
        );
    }

    #[tokio::test]
    async fn both_scope_throttles_when_only_the_global_bucket_is_empty() {
        // Symmetric: drain the GLOBAL bucket (10/s) via Global-scoped calls, then a
        // Both(History) request — whose local side (concurrency) is free — must still
        // throttle, proving the global side is acquired first and gates a Both request.
        let timer = MockTimer::new();
        let svc = layer(timer.clone(), Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        for _ in 0..10 {
            svc.call(req(RateScope::Global)).await.expect("drain global burst 10");
        }
        assert!(
            matches!(
                svc.call(req(RateScope::Both(Key::History))).await.unwrap_err(),
                HttpError::Throttled
            ),
            "Both must acquire the (empty) global bucket before its local side"
        );
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p oath-adapter-net-http-api both_scope_spends_global_and_local_in_one_acquire both_scope_throttles_when_only_the_global_bucket_is_empty`
Expected: **PASS**. **Guard mutation:** if `acquire()` were changed to spend only the local bucket for `Both`, the first test still passes but `both_scope_throttles_when_only_the_global_bucket_is_empty` admits the request → fails. If it spent only global, the first test admits a 3rd → fails.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "test(net): drive RateScope::Both through acquire (global+local order) (M10)"
```

---

### Task 4: Burst over-admission — concurrent acquires vs a burst-B bucket (reasoned-not-observed #2)

**Files:**
- Test: `crates/adapter/net/http/api/src/rate_limit.rs` (append inside `mod tests`)

**Interfaces:** `Key::Snapshot` = burst 2. Fire N simultaneous `acquire()`s (via `tokio::spawn` on cloned services) with `max_wait = 0`; assert **exactly `burst` succeed** and the rest throttle — the token bucket must not momentarily over-admit under a concurrent burst.

- [ ] **Step 1: Write the test**

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_burst_admits_at_most_the_burst_size() {
        // Snapshot burst = 2. Fire 8 requests concurrently against a fresh bucket with
        // max_wait = 0. The bucket must admit EXACTLY 2 and throttle the other 6 — no
        // momentary over-admission from a racing consume/refill.
        let timer = MockTimer::new();
        let svc = layer(timer, Duration::from_secs(0)).layer(Leaf::ok(b"ok"));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = svc.clone();
            handles.push(tokio::spawn(async move {
                s.call(req(RateScope::Local(Key::Snapshot))).await.is_ok()
            }));
        }
        let mut admitted = 0usize;
        for h in handles {
            if h.await.unwrap() {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, 2,
            "a burst-2 bucket admits exactly its burst under a concurrent burst, no over-admission"
        );
    }
```

- [ ] **Step 2: Run** (run several times — concurrency)

Run: `cargo test -p oath-adapter-net-http-api concurrent_burst_admits_at_most_the_burst_size -- --nocapture` (repeat 3×)
Expected: **PASS**, stably `admitted == 2`. **Guard mutation:** a non-atomic check-then-consume in the token bucket (read tokens, then decrement without holding the lock across both) would let >2 through under load → `assert_eq!(admitted, 2)` fails intermittently. If the count is flaky, that is the finding, not a flaky test — investigate the bucket's critical section, do not add retries.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/rate_limit.rs
git commit -m "test(net): pin no burst over-admission under concurrent acquires"
```

---

### Task 5: RateLimit-outside-Timeout ordering (M10-outside-timeout)

**Files:**
- Test: `crates/adapter/net/http/api/src/stack.rs` (append inside `mod tests`, near `send_timeout_fires_on_a_hanging_leaf`, line 565)

**Interfaces (existing, from stack.rs tests):** `stack(leaf, cfg, timer, NoAuth, rate_cfg())`; `http_cfg(retry_attempts, timeout, max_wait)`; `req(RateScope::Local(Key::Snapshot))`; `ScriptLeaf::new(timer, vec![Step::Status(200)])`; `Key::Snapshot` = 2/s burst 2 in `rate_cfg()`; `MockTimer`. **Property:** `RateLimit` sits **outside** `Timeout`, so a permit park is bounded by `rate_limit_max_wait`, **not** the send `timeout`. Assert it by parking a permit for **longer than the send timeout** and showing the request still succeeds.

- [ ] **Step 1: Write the test**

```rust
    // RateLimit sits OUTSIDE Timeout: a permit park is bounded by rate_limit_max_wait,
    // never cut by the (shorter) send timeout. Drain the Snapshot bucket, then a further
    // request parks; advance the clock PAST the 1s send timeout but within the 60s
    // max_wait, refilling a token — the parked request must still succeed (its wait was
    // NOT bounded by the send timeout). If the layers were swapped, the permit wait
    // would inherit the 1s deadline and this would Timeout instead.
    #[tokio::test]
    async fn permit_wait_is_not_bounded_by_the_send_timeout() {
        // RateLimit sits OUTSIDE Timeout: a permit park is bounded by rate_limit_max_wait,
        // never cut by the (much shorter) send timeout. Send timeout = 100ms, max_wait =
        // 60s. Drain Snapshot's 2 tokens, then a 3rd request parks on the empty bucket;
        // advance 500ms — well PAST the 100ms send timeout, within max_wait — to refill a
        // token. The parked request must SUCCEED: its wait was NOT cut by the send
        // timeout. If RateLimit were INSIDE Timeout, the 100ms deadline would fire during
        // the 500ms park and this would be Err(Timeout) instead. (The 500ms park > 100ms
        // timeout is what makes the two arrangements distinguishable — a park shorter than
        // the timeout would pass under both.)
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
        let svc = stack(
            leaf,
            http_cfg(1, Duration::from_millis(100), Duration::from_secs(60)),
            timer.clone(),
            NoAuth,
            rate_cfg(),
        )
        .expect("total config");

        // Drain Snapshot's burst of 2.
        svc.call(req(RateScope::Local(Key::Snapshot))).await.expect("1");
        svc.call(req(RateScope::Local(Key::Snapshot))).await.expect("2");

        // Third parks on the empty bucket. Advance in TWO steps so a swapped-in Timeout
        // is actually observed: first past the 100ms send timeout (150ms) WHILE the park
        // is still blocked (refill only at 500ms), yielding so the task is polled at that
        // tick — under the real (outside) arrangement no timeout applies to the park, so
        // it stays pending; under a swapped (inside) arrangement the 100ms deadline would
        // fire here and resolve the task to Timeout. Then advance the rest to refill and
        // let it succeed. (A single advance(500ms) would NOT catch the swap: both the
        // deadline and the refill become ready at once and select polls the call arm
        // first, letting the park win before the elapsed timeout is seen.)
        let svc2 = svc.clone();
        let waiter =
            tokio::spawn(async move { svc2.call(req(RateScope::Local(Key::Snapshot))).await });
        tokio::task::yield_now().await; // task parks on the empty Snapshot bucket
        timer.advance(Duration::from_millis(150)); // past the 100ms send timeout, before the 500ms refill
        tokio::task::yield_now().await; // poll the task at t=150ms — a swapped-in Timeout would fire NOW
        timer.advance(Duration::from_millis(350)); // total 500ms → refills 1 token
        let resp = waiter
            .await
            .unwrap()
            .expect("parked permit refilled within max_wait — NOT cut by the 100ms send timeout");
        assert_eq!(resp.status(), http::StatusCode::OK);
    }
```

> **Implementer note:** the decisive contrast is that the parked request (which waited 500ms — longer than the 100ms send timeout) resolves to `Ok(200)`, never `Err(Timeout)`. The park MUST exceed the send timeout, or the test can't distinguish RateLimit-outside-Timeout from inside. If the send timeout's own deadline seems to interfere once the permit is finally acquired, remember the send is instant (`Step::Status(200)`), so its fresh 100ms deadline never fires.

- [ ] **Step 2: Run**

Run: `cargo test -p oath-adapter-net-http-api permit_wait_is_not_bounded_by_the_send_timeout`
Expected: **PASS**. **Guard mutation:** swapping `RateLimit` and `Timeout` in `stack()` (permit wait inside the send deadline) makes the parked request time out → `.expect(...)` panics with a `Timeout` error.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/stack.rs
git commit -m "test(net): assert RateLimit-outside-Timeout — permit wait not cut by send timeout (M10)"
```

---

### Task 6: Half-Open + TripNow re-trip on throttle_cooldown (M10-half-open-tripnow)

**Files:**
- Test: `crates/adapter/net/http/api/src/circuit_breaker.rs` (one test in `mod breaker_tests` near line 690; one in `mod service_tests` near line 975)

**Interfaces:**
- `breaker_tests`: `Breaker::new(cfg(threshold, probes))` with fixed `cooldown = 30s`, `throttle_cooldown = 900s`; `b.admit(now) -> Admit::{Pass,Reject,Probe}`; `b.record(Class::{Failure,TripNow,Success}, now)`; `Instant` + `Duration`.
- `service_tests`: `CircuitBreakerLayer::new(cfg(threshold, cooldown, throttle, probes), MockTimer)`; `ScriptLeaf::new(vec![Step::{Err(ErrorKind), Status(u16)}])`; `bare_req()`; `secs(n)`; `timer.advance`.
- **The untested arm:** `circuit_breaker.rs:255` — `HalfOpen + Class::TripNow → Open{reopen_at: now + throttle_cooldown}` (the 900s box), **not** the 30s cooldown a `Failure` probe uses.

- [ ] **Step 1: Write the unit test** (in `mod breaker_tests`)

```rust
    #[test]
    fn half_open_probe_429_reopens_on_the_long_cooldown() {
        // Trip on a normal failure (short 30s cooldown). At Half-Open, the probe returns
        // a 429 (Class::TripNow) — the breaker must re-open on throttle_cooldown (900s),
        // NOT the 30s cooldown a failing probe would use. Distinguish the two: at
        // reopen+30s still Reject; only at reopen+900s does the next probe admit.
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // trips on the first failure
        b.record(Class::Failure, now); // Closed → Open (30s cooldown)
        let probe_at = now + Duration::from_secs(30);
        assert_eq!(b.admit(probe_at), Admit::Probe, "cooldown elapsed → probe");
        b.record(Class::TripNow, probe_at); // 429 during the probe → long box
        assert_eq!(
            b.admit(probe_at + Duration::from_secs(30)),
            Admit::Reject,
            "short cooldown is NOT enough after a 429 re-trip"
        );
        assert_eq!(
            b.admit(probe_at + Duration::from_secs(900)),
            Admit::Probe,
            "only throttle_cooldown reopens after a Half-Open 429 re-trip"
        );
    }
```

- [ ] **Step 2: Write the service-level test** (in `mod service_tests`, mirrors `a_single_429_trips_immediately_on_the_long_cooldown`)

```rust
    #[tokio::test]
    async fn a_429_during_a_half_open_probe_reopens_on_the_long_cooldown() {
        let timer = MockTimer::new();
        // fail, fail → Open (30s); probe returns 429 → reopen on throttle_cooldown (900s);
        // final probe returns 200.
        let leaf = ScriptLeaf::new(vec![
            Step::Err(ErrorKind::Connection),
            Step::Err(ErrorKind::Connection),
            Step::Status(429),
            Step::Status(200),
        ]);
        let svc = CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone())
            .layer(leaf.clone());
        let _ = svc.call(bare_req()).await; // fail 1
        let _ = svc.call(bare_req()).await; // fail 2 → Open
        assert!(matches!(
            svc.call(bare_req()).await.unwrap_err(),
            HttpError::CircuitOpen
        ));
        timer.advance(secs(30)); // normal cooldown → probe admitted
        let resp = svc.call(bare_req()).await.expect("probe reaches the leaf");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS); // 429 as Ok
        // Re-opened on the LONG cooldown: 30s more is not enough…
        timer.advance(secs(30));
        assert!(
            matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen),
            "a 429 probe re-opened on throttle_cooldown, not the 30s cooldown"
        );
        // …but a further advance to a full 900s from the re-trip admits the next probe.
        timer.advance(secs(870)); // 30 + 870 = 900 total since the 429 re-trip
        let ok = svc.call(bare_req()).await.expect("throttle_cooldown elapsed → probe → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
    }
```

- [ ] **Step 3: Run both**

Run: `cargo test -p oath-adapter-net-http-api half_open_probe_429_reopens_on_the_long_cooldown a_429_during_a_half_open_probe_reopens_on_the_long_cooldown`
Expected: **PASS**. **Guard mutation:** if `circuit_breaker.rs:255` used `cooldown` instead of `throttle_cooldown` for the Half-Open+TripNow arm, both tests admit a probe 30s after the re-trip → the `Reject`/`CircuitOpen` assertions fail.

> **Implementer note:** confirm the exact `throttle_cooldown` accounting for the service test by reading `circuit_breaker.rs` `record()` (the reopen deadline may be measured from the probe instant). If the `secs(870)` split doesn't land the probe, adjust so the total elapsed since the 429 equals `throttle_cooldown` — the invariant to preserve is "reject at +30s, admit at +900s from the re-trip."

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs
git commit -m "test(net): pin Half-Open 429 re-trip on throttle_cooldown, not cooldown (M10)"
```

---

### Task 7: Retry backoff schedule doubling pin (L-backoff-schedule)

**Files:**
- Test: `crates/adapter/net/http/api/src/retry.rs` (extend `backoff_ceiling_clamps_and_saturates` in `mod tests`, line 733 — or add a sibling test)

**Interfaces:** `super::backoff_ceiling(base: Duration, cap: Duration, attempt: u32) -> Duration`. Existing asserts only cover `attempt==1→base`, a clamped `attempt==3`, `cap<base`, and saturation — never an **un-clamped** `2·base`/`4·base`. Add the doubling ladder with `cap` set high enough that no clamp fires.

- [ ] **Step 1: Add the test** (sibling to the existing one)

```rust
    #[test]
    fn backoff_ceiling_doubles_each_attempt_until_the_cap() {
        // base = 10ms, cap = 10s (high enough that clamping never fires for attempts
        // 1..=4): the ceiling must be base·2^(attempt-1) — 10, 20, 40, 80 ms. Every
        // loop-driving retry test uses base == cap, so the doubling law is otherwise
        // unpinned; a 2^attempt (overshoot) or "no doubling" bug lands here.
        let base = Duration::from_millis(10);
        let cap = Duration::from_secs(10);
        assert_eq!(super::backoff_ceiling(base, cap, 1), Duration::from_millis(10));
        assert_eq!(super::backoff_ceiling(base, cap, 2), Duration::from_millis(20));
        assert_eq!(super::backoff_ceiling(base, cap, 3), Duration::from_millis(40));
        assert_eq!(super::backoff_ceiling(base, cap, 4), Duration::from_millis(80));
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p oath-adapter-net-http-api backoff_ceiling_doubles_each_attempt_until_the_cap`
Expected: **PASS**. **Guard mutation:** `2^attempt` instead of `2^(attempt-1)` yields 20/40/80/160 → the `attempt==1` assert (expects 10ms) fails; dropping the doubling (constant `base`) yields 10/10/10/10 → the `attempt==2` assert fails.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs
git commit -m "test(net): pin the Retry backoff doubling ladder (unclamped 2^(n-1))"
```

---

### Task 8: SplitMix64 golden-vector guard (splitmix64-golden-vector)

**Files:**
- Test: `crates/adapter/net/http/api/src/retry.rs` (append inside `mod rng_tests`, near line 763)

**Interfaces:** `mod rng_tests` already does `use super::SplitMix64;` — as a child module it can call the **private** `SplitMix64::next_u64` and `::new`. This impl matches canonical SplitMix64 (`next()` finalizes `seed + STEP`), so `seed = 0` yields the published reference sequence. Determinism is already covered (`same_seed_reproduces_the_same_sequence`), but that does **not** catch algorithm drift (two drifted instances still agree). A fixed absolute-output vector does.

- [ ] **Step 1: Write the test** (canonical seed-0 reference vector)

```rust
    #[test]
    fn splitmix64_matches_the_reference_golden_vector() {
        // Absolute reference vector from https://prng.di.unimi.it/splitmix64.c for
        // seed = 0: next() finalizes (state += GOLDEN) each call. Guards against silent
        // algorithm drift — a changed STEP or finalizer constant breaks deterministic
        // backoff replay, which a same-seed determinism test can NOT catch.
        let rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p oath-adapter-net-http-api splitmix64_matches_the_reference_golden_vector`
Expected: **PASS**.

> **Implementer verification (important):** if this *fails* on first run, do **not** blindly replace the constants with the observed output — a mismatch means either (a) genuine algorithm drift (the bug this test exists to catch) or (b) a transcription slip in the reference literals. Cross-check the three values against the reference `splitmix64.c` seed-0 output before deciding. Only pin observed values if you have confirmed the algorithm is the unmodified reference.

**Guard mutation:** changing `STEP` or either finalizer multiplier in `SplitMix64` changes all three draws → every assert fails.

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs
git commit -m "test(net): add a SplitMix64 golden-vector drift guard"
```

---

## Group B — Integration over the real hyper leaf

### Task 9: Real-leaf resilience integration tests (integration-real-leaf-resilience)

**Files:**
- Test: `crates/adapter/net/http/hyper/src/build.rs` (extend `mod tests`, after line 174)

**Interfaces (existing in build.rs tests):** `build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())`; `conn()` (has `allow_http: true`); `Key::Rest`; `spawn_echo()`; `RateScope::<Key>::Global`; `TokioTimer` (real clock). These integration tests use **real time**, so keep `retry.base == retry.cap == 0` (already true in `http_cfg()`), and use short real `Duration`s for timeouts. Add two loopback-server helpers and the tests.

**Why here:** `net-http-hyper` prod-depends on `oath-adapter-net-http-api::stack`; `net-http-api` has no hyper dep → no cycle. This is the only crate that can drive the assembled stack over the *real* leaf. Today only a happy-path smoke test exists.

- [ ] **Step 1: Add server helpers** (after `spawn_echo`, ~line 131)

```rust
    // A server that fails the FIRST connection (accept then drop → connection reset)
    // and echoes "ok" on every subsequent one — for the retry-over-a-real-reset test.
    async fn spawn_fail_then_ok() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                if first {
                    first = false;
                    drop(stream); // reset the first connection before any response
                    continue;
                }
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(|_r| async {
                        Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                            Bytes::from_static(b"ok"),
                        )))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    // A server that always replies with a fixed status code (empty body).
    async fn spawn_status(code: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |_r| async move {
                        let mut resp = hyper::Response::new(http_body_util::Full::new(Bytes::new()));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok::<_, Infallible>(resp)
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    // Build a request with an explicit Global scope + the Retryable opt-in.
    fn req_retryable(url: String) -> http::Request<Bytes> {
        let mut r = http::Request::get(url).body(Bytes::new()).unwrap();
        r.extensions_mut().insert(RateScope::<Key>::Global);
        r.extensions_mut().insert(oath_adapter_net_http_api::Retryable);
        r
    }
```

> Add `use oath_adapter_net_http_api::Retryable;` alongside the existing imports if you prefer a bare name; the fully-qualified path above also works.

- [ ] **Step 2: Add the integration tests**

```rust
    // A real dropped connection on the first attempt maps to HttpError::Connection
    // (H1/H2), which Retry treats as transient — the 2nd attempt reaches the healthy
    // server and returns 200. Observes end-to-end what stack.rs only reasons about.
    #[tokio::test]
    async fn a_real_connection_reset_is_retried_over_the_hyper_leaf() {
        let base = spawn_fail_then_ok().await;
        // retry ON (2 attempts), zero backoff (real clock, no wait).
        let mut cfg = http_cfg();
        cfg.retry.max_attempts = NonZeroU32::new(2).unwrap();
        let client = build(cfg, TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");
        let resp = client
            .call(req_retryable(format!("{base}/x")))
            .await
            .expect("2nd attempt after a real reset succeeds");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok"));
    }

    // A real venue 429 arrives as Ok(status = 429). With the breaker threshold reached
    // it trips the circuit; the next call fast-rejects with CircuitOpen without a send.
    #[tokio::test]
    async fn a_real_429_trips_the_breaker_through_the_full_stack() {
        let base = spawn_status(429).await;
        let client = build(http_cfg(), TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");
        // One 429 trips immediately on the long cooldown (throttle path).
        let resp = client
            .call(req_retryable(format!("{base}/x")))
            .await
            .expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        let Err(err) = client.call(req_retryable(format!("{base}/x"))).await else {
            panic!("expected CircuitOpen after a 429 trip");
        };
        assert!(matches!(err, oath_adapter_net_http_api::HttpError::CircuitOpen));
    }

    // The Timeout layer bounds a real send: a server that never responds (accept +
    // hold) yields HttpError::Timeout at a short real send timeout.
    #[tokio::test]
    async fn send_timeout_fires_over_a_real_hanging_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (_stream, _) = listener.accept().await.unwrap();
                // Hold the connection open, never respond.
                std::future::pending::<()>().await;
            }
        });
        let base = format!("http://{addr}");
        let mut cfg = http_cfg();
        cfg.timeout = Duration::from_millis(200); // short real send bound
        let client = build(cfg, TokioTimer, NoAuth, total_rates(), conn())
            .expect("total config builds");
        let Err(err) = client.call(req_retryable(format!("{base}/x"))).await else {
            panic!("expected Timeout from a hanging server");
        };
        assert!(matches!(err, oath_adapter_net_http_api::HttpError::Timeout));
    }
```

> **Implementer note:** `a_real_429_trips_the_breaker_through_the_full_stack` assumes the default breaker path trips a single 429 immediately (the throttle/`TripNow` path, confirmed in `circuit_breaker.rs` and its unit tests). If `http_cfg()`'s `failure_threshold` (3) applies to 429s in the assembled stack instead, drive three 429s before asserting `CircuitOpen`. Verify against the breaker classification (`classify` maps status-429 → `TripNow`, which trips immediately regardless of threshold). Keep real `Duration`s conservative to avoid CI flake.

- [ ] **Step 3: Run**

Run: `cargo test -p oath-adapter-net-http-hyper --  a_real_connection_reset_is_retried_over_the_hyper_leaf a_real_429_trips_the_breaker_through_the_full_stack send_timeout_fires_over_a_real_hanging_server`
Expected: **PASS**. **Guard value:** these observe H1/H2 (reset → `Connection` → retried) and the breaker trip over the *real* transport, not a mock — the exact gap the deep-review flagged.

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/hyper/src/build.rs
git commit -m "test(net): integration-test the resilience stack over the real hyper leaf"
```

---

### Task 10: HTTP/2 keepalive positive test + documented defer of reaping (h2-keepalive)

**Files:**
- Test: `crates/adapter/net/http/hyper/src/leaf.rs` (append inside the leaf `#[cfg(test)]` module)
- Doc: a `//` comment in `crates/adapter/net/http/hyper/src/leaf.rs` recording the deferred negative case; a checkbox note appended to the `net-http-audit-findings.md` / deep-review defer log (Task 15 aggregates defer notes).

**Decision (confirmed):** implement the **positive** case — a keepalive-configured HTTP/2 connection survives an idle gap and serves a second request over the same pooled connection — and **document-defer** the negative "reaped-without-keepalive" case (it depends on hyper/OS idle timing and is flake-prone). All current test servers are HTTP/1.1; HTTP/2 over the pooled leaf requires ALPN `h2`, so this test needs a TLS h2 server.

**Interfaces:** reuse the self-signed-cert setup pattern from `hyper_leaf_round_trips_over_tls_with_a_custom_root` (leaf.rs:493) — `rcgen::generate_simple_self_signed`, `tokio_rustls::TlsAcceptor`, `TlsTrust::CustomRoots(vec![cert_der])`. Serve with `hyper::server::conn::http2::Builder::new(TokioExecutor::new())` and a rustls `ServerConfig` whose `alpn_protocols = vec![b"h2".to_vec()]`.

- [ ] **Step 1: Read the existing TLS test verbatim** to reuse its cert/acceptor scaffolding:

Run: `sed -n '480,565p' crates/adapter/net/http/hyper/src/leaf.rs`
Note the exact `rcgen`/`tokio_rustls`/`ServerConfig` construction and the `CustomRoots` wiring; the h2 test differs only in (a) ALPN `h2` on the server config, (b) an `http2` server builder with a `TokioExecutor`, and (c) `ConnConfig.http2_keep_alive_interval = Some(...)`.

- [ ] **Step 2: Write the positive keepalive test**

```rust
    // HTTP/2 keepalive (positive case): with http2_keep_alive_interval set and
    // while_idle = true, a pooled h2 connection survives a brief idle gap and serves a
    // second request over the SAME connection. The negative "reaped without keepalive"
    // case is deferred (hyper/OS idle-timing dependent, flake-prone) — see the defer
    // note below. This asserts the keepalive knobs thread through and an idle h2
    // connection stays usable.
    #[tokio::test]
    async fn h2_keepalive_connection_survives_an_idle_gap() {
        // --- self-signed cert + rustls server config with ALPN "h2" (mirror the TLS
        //     round-trip test's scaffolding; add alpn_protocols) ---
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = rustls_pki_types::CertificateDer::from(cert.cert.der().to_vec());
        let key_der = rustls_pki_types::PrivateKeyDer::try_from(
            cert.key_pair.serialize_der(),
        )
        .unwrap();
        let mut server_cfg = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .unwrap();
        server_cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server_cfg));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls = acceptor.accept(tcp).await.unwrap();
                    let io = hyper_util::rt::TokioIo::new(tls);
                    let svc = hyper::service::service_fn(|_r| async {
                        Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                            Bytes::from_static(b"h2ok"),
                        )))
                    });
                    let _ = hyper::server::conn::http2::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, svc)
                    .await;
                });
            }
        });

        // --- leaf with custom root + keepalive enabled ---
        let conn = ConnConfig {
            tls_trust: TlsTrust::CustomRoots(vec![cert_der]),
            allow_http: false,
            http2_keep_alive_interval: Some(Duration::from_millis(50)),
            http2_keep_alive_timeout: Duration::from_secs(5),
            http2_keep_alive_while_idle: true,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);

        let url = format!("https://localhost:{}/x", addr.port());
        // First request establishes the h2 connection.
        let r1 = leaf
            .call(http::Request::get(&url).body(Bytes::new()).unwrap())
            .await
            .expect("first h2 request");
        assert_eq!(r1.status(), http::StatusCode::OK);
        let b1 = r1.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(b1, Bytes::from_static(b"h2ok"));

        // Idle longer than several keepalive intervals; the PING keeps the connection
        // alive. A second request must still succeed over the pooled h2 connection.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let r2 = leaf
            .call(http::Request::get(&url).body(Bytes::new()).unwrap())
            .await
            .expect("second h2 request over a kept-alive connection");
        assert_eq!(r2.status(), http::StatusCode::OK);
    }
```

> **Implementer notes:**
> - The cert/key construction must match the crate's `rcgen`/`rustls_pki_types` versions exactly — **copy the working lines from `hyper_leaf_round_trips_over_tls_with_a_custom_root`** (leaf.rs:493) and add only `server_cfg.alpn_protocols`. Resolving the connection to `localhost` (not the `127.0.0.1` IP) matters for SNI/cert-name matching — the existing TLS test shows the exact URL/host convention; follow it.
> - This is an honest **positive/plumbing** assertion (keepalive configured → idle h2 connection stays usable), not a PING-frame observation. That is the intended scope per the confirmed decision.
> - If ALPN negotiation or the pooled h2 reuse proves environment-fragile in CI, mark the test `#[ignore = "h2 keepalive: environment-timing sensitive; see defer note"]` **and** record it in the defer note (Step 3) rather than deleting it — but try to land it running first.

- [ ] **Step 3: Add the defer note** (a `//` comment right above the test)

```rust
    // Deferred (Tier-1 → tracking): the NEGATIVE h2-keepalive case — an idle h2
    // connection being REAPED when keepalive is disabled — is not observed here. It
    // depends on hyper's/OS idle-connection timing and is flake-prone as a unit test;
    // the keepalive config knobs and the positive survival path are covered above.
```

- [ ] **Step 4: Run**

Run: `cargo test -p oath-adapter-net-http-hyper h2_keepalive_connection_survives_an_idle_gap -- --nocapture` (repeat 3×)
Expected: **PASS**, stably. If flaky, apply the `#[ignore]` fallback from the note and proceed.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/hyper/src/leaf.rs
git commit -m "test(net): positive HTTP/2 keepalive survival test; defer the reaping case"
```

---

## Group C — Docs & hygiene

### Task 11: Replace the L12 tautological tests (L12)

**Files:**
- Modify: `crates/adapter/net/http/api/src/rate.rs` (tests at lines 241-275 and 395-411)

**Rationale:** `config_classifies_every_key_explicitly` (rate.rs:242) only re-reads literals it just built and calls no production fn; the real coverage lives in `total_config_validates`, `missing_key_is_undeclared`, and `rate_key_all_is_drift_proof`. `token_bucket_carries_a_period_for_sub_1_per_second_rates` (rate.rs:396) matches a literal it constructed and discards the `per` field with `..`. **Fix:** delete the first (fully redundant); rewrite the second to assert something real — that `validate_coverage` accepts a sub-1/s token bucket as a valid global policy (the behavior the name implies).

- [ ] **Step 1: Delete `config_classifies_every_key_explicitly`** (rate.rs:241-275). Remove the whole `#[test] fn config_classifies_every_key_explicitly() { … }` block.

- [ ] **Step 2: Replace `token_bucket_carries_a_period_for_sub_1_per_second_rates`** with a real assertion:

```rust
    #[test]
    fn a_sub_one_per_second_token_bucket_is_a_valid_policy() {
        // A 1-token-per-5s bucket (sub-1/s) is param-sane and must pass validation —
        // the period carries the sub-Hz rate. Exercises LimitPolicy::validate via
        // validate_coverage, not a literal re-read.
        let cfg = RateLimitConfig::<TestKey> {
            global: LimitPolicy::TokenBucket {
                rate: 1,
                per: Duration::from_secs(5),
                burst: 1,
            },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::GlobalOnly),
                (TestKey::Snapshot, LimitDecl::GlobalOnly),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(validate_coverage(&cfg), Ok(()));
    }
```

> **Implementer note:** confirm `TestKey::all()` variants (the test module defines `PlaceOrder`, `Snapshot`, `History` — see rate.rs:238 `assert_eq!(TestKey::all().len(), 3)`). The `local` map must be total over them or `validate_coverage` returns `UndeclaredKey`. Adjust the variant list if the enum differs.

- [ ] **Step 3: Run**

Run: `cargo test -p oath-adapter-net-http-api --lib rate::` (or the module's test path)
Expected: **PASS**; the deleted test is gone, the replacement passes. Confirm no other test referenced the deleted fn (it was self-contained).

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/rate.rs
git commit -m "test(net): replace tautological rate-config tests with real validation checks (L12)"
```

---

### Task 12: Fix stale rustdoc (L7, L8)

**Files:**
- Modify: `crates/adapter/net/http/api/src/lib.rs:26-30`
- Modify: `crates/adapter/net/http/api/src/rate.rs:157-158`

- [ ] **Step 1: Fix the crate-level doc (L7).** In `lib.rs`, replace the stale trailer. Change lines 26-30 from:

```rust
//! - [`stack()`] — `HttpConfig` and the `stack()` assembly composing the canonical
//!   resilience order (ADR-0031 §1) over any leaf (Slice 2)
//!
//! The resilience layers, `stack`/`build` assembly, and backends land in later
//! slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
```

to:

```rust
//! - [`stack()`] — `HttpConfig` and the `stack()` assembly composing the canonical
//!   resilience order (ADR-0031 §1) over any leaf
//!
//! The resilience layers and `stack()` assembly live here; the `hyper` backend and
//! `build()` construction surface live in `oath-adapter-net-http-hyper`. No async
//! runtime, `hyper`, `reqwest`, or `serde` in this crate.
```

- [ ] **Step 2: Fix the `validate_coverage` caller doc (L8).** In `rate.rs`, change lines 157-158 from:

```rust
/// with a valid policy (ADR-0034 §3). Slice 2's `stack()`/`build()` call this
/// before assembling the stack, so a coverage gap is a boot failure.
```

to (matching the already-correct `validate_concurrency_singleton` doc at rate.rs:182):

```rust
/// with a valid policy (ADR-0034 §3). `RateLimitLayer::new` calls this (and
/// `stack()` transitively) before assembling the stack, so a coverage gap is a
/// boot failure.
```

- [ ] **Step 3: Verify docs build clean**

Run: `just doc`
Expected: **PASS**, no broken intra-doc links (the `[`RateLimitLayer`]`/`[`validate_coverage`]` links must resolve). If `RateLimitLayer` isn't in scope for an intra-doc link from `rate.rs`, use the plain name without brackets or a fully-qualified `[`crate::rate_limit::RateLimitLayer`]`.

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/api/src/lib.rs crates/adapter/net/http/api/src/rate.rs
git commit -m "docs(net): correct stale 'later slices' + validator caller rustdoc (L7/L8)"
```

---

### Task 13: Doctests on the public API (L13)

**Files:**
- Modify (add `///` doctests): `stack.rs:77` (`stack`), `client.rs:12` (`HttpClient`), `rate_limit.rs:23` (`RateScope`) + `:132` (`RateLimitLayer::new`), `retry.rs:146` (`RetryLayer::new`), `timeout.rs:49` (`TimeoutLayer::new`), `circuit_breaker.rs:322` (`CircuitBreakerLayer::new`), `trace.rs:53` (`TracingLayer::new`), `build.rs:22` (`build`)
- Modify: `crates/adapter/net/http/api/Cargo.toml` and `crates/adapter/net/http/hyper/Cargo.toml` — ensure `http` and `bytes` are available to doctests.

**Key constraint:** rustdoc doctests link against the crate + its **dev-dependencies** (not its normal deps unless also dev-deps). Constructing an `http::Request` in a doctest therefore needs `http` (and `bytes`) as **dev-dependencies**. `MockTimer` (from `oath-adapter-net-mock`) is already a dev-dep of `net-http-api`.

- [ ] **Step 1: Make `http`/`bytes` importable in doctests.** In `crates/adapter/net/http/api/Cargo.toml` `[dev-dependencies]`, add (workspace-versioned like the existing deps):

```toml
http = { workspace = true }
bytes = { workspace = true }
```

Do the same in `crates/adapter/net/http/hyper/Cargo.toml` `[dev-dependencies]` if not already present (it uses `bytes`/`http` in tests — confirm both are listed there; add whichever is missing).

**As-built note:** this step was found unnecessary and not done. `http`/`bytes` are already normal (non-dev) dependencies of both crates, and rustdoc doctests link against a crate's normal dependencies as well as its dev-dependencies — so they resolved without any `Cargo.toml` change.

- [ ] **Step 2: Verify the assumption with the smallest doctest first.** Add this to `RateScope` (rate_limit.rs, above the `pub enum RateScope`, ~line 23):

```rust
/// # Example
/// Stamp the mandatory per-request pacing directive before calling the client
/// (an absent `RateScope` fails closed with [`HttpError::Throttled`]):
/// ```
/// use oath_adapter_net_http_api::RateScope;
///
/// #[derive(Clone, Copy)]
/// enum Endpoint { Orders }
///
/// let mut req = http::Request::new(bytes::Bytes::new());
/// req.extensions_mut().insert(RateScope::Local(Endpoint::Orders));
/// assert!(req.extensions().get::<RateScope<Endpoint>>().is_some());
/// ```
```

Run: `cargo test -p oath-adapter-net-http-api --doc`
Expected: **PASS** (proves the dev-dep wiring works). If `http`/`bytes` don't resolve, the `[dev-dependencies]` entries from Step 1 are missing or misnamed — fix before proceeding.

- [ ] **Step 3: Add the layer-factory doctests.** Each uses `MockTimer` (dev-dep) as the `Timer`. Add above each `new`:

`RateLimitLayer::new` (rate_limit.rs:132):
```rust
/// # Example
/// ```
/// use oath_adapter_net_http_api::{RateLimitConfig, RateLimitLayer, LimitDecl, LimitPolicy, RateKey};
/// use oath_adapter_net_mock::MockTimer;
/// use std::collections::HashMap;
/// use std::time::Duration;
///
/// #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
/// enum Endpoint { Orders }
/// impl RateKey for Endpoint { fn all() -> &'static [Self] { &[Endpoint::Orders] } }
///
/// let cfg = RateLimitConfig {
///     global: LimitPolicy::TokenBucket { rate: 10, per: Duration::from_secs(1), burst: 10 },
///     local: HashMap::from([(Endpoint::Orders, LimitDecl::GlobalOnly)]),
/// };
/// let layer = RateLimitLayer::new(&cfg, MockTimer::new(), Duration::from_secs(0));
/// assert!(layer.is_ok());
/// ```
```

`RetryLayer::new` (retry.rs:146):
```rust
/// # Example
/// ```
/// use oath_adapter_net_http_api::{RetryLayer, RetryConfig};
/// use oath_adapter_net_mock::MockTimer;
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// let cfg = RetryConfig {
///     max_attempts: NonZeroU32::new(3).unwrap(),
///     base: Duration::from_millis(50),
///     cap: Duration::from_secs(1),
///     seed: 1,
/// };
/// let _layer = RetryLayer::new(cfg, MockTimer::new());
/// ```
```

`TimeoutLayer::new` (timeout.rs:49):
```rust
/// # Example
/// ```
/// use oath_adapter_net_http_api::TimeoutLayer;
/// use oath_adapter_net_mock::MockTimer;
/// use std::time::Duration;
///
/// let _layer = TimeoutLayer::new(Duration::from_secs(5), MockTimer::new());
/// ```
```

`CircuitBreakerLayer::new` (circuit_breaker.rs:322):
```rust
/// # Example
/// ```
/// use oath_adapter_net_http_api::{CircuitBreakerLayer, CircuitBreakerConfig};
/// use oath_adapter_net_mock::MockTimer;
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// let cfg = CircuitBreakerConfig {
///     failure_threshold: NonZeroU32::new(3).unwrap(),
///     cooldown: Duration::from_secs(30),
///     throttle_cooldown: Duration::from_secs(900),
///     half_open_probes: NonZeroU32::new(1).unwrap(),
/// };
/// let _layer = CircuitBreakerLayer::new(cfg, MockTimer::new());
/// ```
```

`TracingLayer::new` (trace.rs:53):
```rust
/// # Example
/// ```
/// use oath_adapter_net_http_api::TracingLayer;
/// use oath_adapter_net_mock::MockTimer;
///
/// let _layer = TracingLayer::new(MockTimer::new());
/// ```
```

- [ ] **Step 4: Add the `HttpClient` and `stack()` doctests** (api crate). For `HttpClient` (client.rs:12), a short illustrative doctest of the `send` seam — use `no_run` if it needs a concrete client, or a trait-object sketch. Prefer a compiling example that names the trait:

```rust
/// # Example
/// The `HttpClient` seam is what adapters depend on — construct one with
/// [`stack()`] or the hyper `build()` and call it through this trait:
/// ```no_run
/// use oath_adapter_net_http_api::HttpClient;
/// use bytes::Bytes;
///
/// async fn fetch(client: &impl HttpClient, req: http::Request<Bytes>) {
///     let _ = client.call(req).await;
/// }
/// ```
```

For `stack()` (stack.rs:77) — a full assembly doctest is heavy; use `no_run` and reuse the config shapes. Prefer building a leaf-agnostic example against a trivial inline leaf **or** mark `no_run` and reference `build()`:

```rust
/// # Example
/// Assemble the canonical resilience stack over any leaf `Service`. In production the
/// hyper backend's `build()` wraps this; see `oath-adapter-net-http-hyper::build`.
/// ```no_run
/// # // A full worked example (with a concrete leaf + config) lives in
/// # // `oath-adapter-net-http-hyper`'s `examples/` and `build()` doctest.
/// ```
```

> **Implementer note:** if an empty `no_run` block trips the "empty doctest" lint, inline the minimal config construction from the `RateLimitLayer::new` example plus a one-line leaf stub, or drop the `stack()` doctest and rely on the `build()` doctest (Step 5) + the worked example (Task 14) for the assembly path. `missing_docs` is already satisfied by the existing prose; the doctest is additive.

- [ ] **Step 5: Add the `build()` doctest** (hyper crate, build.rs:22) — `no_run` (it would bind a socket / need a live server otherwise):

```rust
/// # Example
/// ```no_run
/// use oath_adapter_net_http_hyper::{build, ConnConfig, TlsTrust, TokioTimer};
/// use oath_adapter_net_http_api::{
///     HttpConfig, NoAuth, RateKey, RateLimitConfig, LimitDecl, LimitPolicy,
///     RetryConfig, CircuitBreakerConfig,
/// };
/// use std::collections::HashMap;
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// enum Endpoint { Rest }
/// impl RateKey for Endpoint { fn all() -> &'static [Self] { &[Endpoint::Rest] } }
///
/// let cfg = HttpConfig {
///     timeout: Duration::from_secs(5),
///     retry: RetryConfig { max_attempts: NonZeroU32::new(3).unwrap(), base: Duration::from_millis(50), cap: Duration::from_secs(1), seed: 1 },
///     circuit_breaker: CircuitBreakerConfig { failure_threshold: NonZeroU32::new(3).unwrap(), cooldown: Duration::from_secs(30), throttle_cooldown: Duration::from_secs(900), half_open_probes: NonZeroU32::new(1).unwrap() },
///     headers: http::HeaderMap::new(),
///     rate_limit_max_wait: Duration::from_secs(0),
/// };
/// let rates = RateLimitConfig {
///     global: LimitPolicy::TokenBucket { rate: 1000, per: Duration::from_secs(1), burst: 1000 },
///     local: HashMap::from([(Endpoint::Rest, LimitDecl::GlobalOnly)]),
/// };
/// let conn = ConnConfig {
///     pool_max_idle_per_host: 4,
///     pool_idle_timeout: Duration::from_secs(30),
///     connect_timeout: Duration::from_secs(2),
///     tls_trust: TlsTrust::WebpkiRoots,
///     allow_http: false,
///     http2_keep_alive_interval: None,
///     http2_keep_alive_timeout: Duration::from_secs(10),
///     http2_keep_alive_while_idle: false,
/// };
/// let _client = build(cfg, TokioTimer, NoAuth, rates, conn).expect("valid config");
/// ```
```

> **Implementer note:** verify each imported name's exact re-export path (`ConnConfig`, `TlsTrust`, `TokioTimer` are re-exported from the hyper crate root — confirm with `grep 'pub use' crates/adapter/net/http/hyper/src/lib.rs`; `HttpConfig`/configs from `oath-adapter-net-http-api`). Fix any path that doesn't resolve. Keep it `no_run`.

- [ ] **Step 6: Run all doctests + doc build**

Run: `cargo test -p oath-adapter-net-http-api --doc && cargo test -p oath-adapter-net-http-hyper --doc && just doc`
Expected: **PASS**, all doctests compile/run, `just doc` clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/net/http/api crates/adapter/net/http/hyper
git commit -m "docs(net): add doctests for stack/build/HttpClient/RateScope/layer factories (L13)"
```

---

### Task 14: Worked example + README for the per-request extension protocol

**Files:**
- Create: `crates/adapter/net/http/hyper/examples/client_with_directives.rs`
- Create: `crates/adapter/net/http/hyper/README.md`

**Rationale (spec):** the mandatory per-request extension protocol (`RateScope` mandatory/fail-closed, `Retryable` opt-in, `BufferMode`) is documented only per-type today; a forgotten `RateScope` stamp is the most likely real-world C1 trigger. Examples compile with dev-deps, so `#[tokio::main]` and a loopback server are available.

- [ ] **Step 1: Write the example** — builds a client, spins a local echo server, stamps all three directives, sends a request:

```rust
//! A worked example of the mandatory per-request extension protocol for the net-http
//! stack: every request MUST carry an explicit `RateScope` (fail-closed — an absent
//! scope is rejected as `Throttled`, never sent); `Retryable` opts a request into the
//! Retry layer; `BufferMode` chooses streaming vs buffered response bodies.
//!
//! Run with: `cargo run -p oath-adapter-net-http-hyper --example client_with_directives`

use bytes::Bytes;
use http_body_util::BodyExt;
use oath_adapter_net_http_api::{
    BufferMode, CircuitBreakerConfig, HttpConfig, LimitDecl, LimitPolicy, NoAuth, RateKey,
    RateLimitConfig, RateScope, Retryable, RetryConfig, Service,
};
use oath_adapter_net_http_hyper::{build, ConnConfig, TlsTrust, TokioTimer};
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;
use std::time::Duration;
use tokio::net::TcpListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Endpoint {
    Rest,
}
impl RateKey for Endpoint {
    fn all() -> &'static [Self] {
        &[Endpoint::Rest]
    }
}

#[tokio::main]
async fn main() {
    // A local plaintext echo server stands in for the venue.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = hyper_util::rt::TokioIo::new(stream);
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(|_r| async {
                    Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                        Bytes::from_static(b"pong"),
                    )))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await;
            });
        }
    });

    let cfg = HttpConfig {
        timeout: Duration::from_secs(5),
        retry: RetryConfig {
            max_attempts: NonZeroU32::new(3).unwrap(),
            base: Duration::from_millis(50),
            cap: Duration::from_secs(1),
            seed: 1,
        },
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(3).unwrap(),
            cooldown: Duration::from_secs(30),
            throttle_cooldown: Duration::from_secs(900),
            half_open_probes: NonZeroU32::new(1).unwrap(),
        },
        headers: http::HeaderMap::new(),
        rate_limit_max_wait: Duration::from_secs(0),
    };
    let rates = RateLimitConfig {
        global: LimitPolicy::TokenBucket {
            rate: 1000,
            per: Duration::from_secs(1),
            burst: 1000,
        },
        local: HashMap::from([(Endpoint::Rest, LimitDecl::GlobalOnly)]),
    };
    let conn = ConnConfig {
        pool_max_idle_per_host: 4,
        pool_idle_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(2),
        tls_trust: TlsTrust::WebpkiRoots,
        allow_http: true, // local plaintext gateway
        http2_keep_alive_interval: None,
        http2_keep_alive_timeout: Duration::from_secs(10),
        http2_keep_alive_while_idle: false,
    };

    let client = build(cfg, TokioTimer, NoAuth, rates, conn).expect("valid config");

    // ---- the per-request extension protocol ----
    let mut req = http::Request::get(format!("http://{addr}/quotes"))
        .body(Bytes::new())
        .unwrap();
    // MANDATORY: an explicit pacing scope. Omit it and the stack fails closed (Throttled).
    req.extensions_mut().insert(RateScope::Global);
    // OPTIONAL: opt into retries for this (idempotent) request.
    req.extensions_mut().insert(Retryable);
    // OPTIONAL: buffer the whole body inside the retry boundary (default is Stream).
    req.extensions_mut().insert(BufferMode::Buffer);

    let resp = client.call(req).await.expect("round-trip");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    println!("venue said: {}", String::from_utf8_lossy(&body));
    assert_eq!(body, Bytes::from_static(b"pong"));
}
```

- [ ] **Step 2: Write the README** — `crates/adapter/net/http/hyper/README.md`:

```markdown
# oath-adapter-net-http-hyper

The hyper backend for the OATH net-http stack: `build()` assembles the canonical
resilience stack (rate-limit → retry → circuit-breaker → timeout → tracing) over a
pooled `hyper_util` client with a rustls HTTPS connector.

## The per-request extension protocol

Every request carries its resilience directives as `http::Extensions`. Stamp them
with `req.extensions_mut().insert(..)` **before** calling the client:

| Directive | Required? | Absent default | Purpose |
|---|---|---|---|
| `RateScope<K>` | **Yes** (fail-closed) | rejected as `Throttled` — never sent | Which pacing bucket(s) to spend: `None` / `Global` / `Local(k)` / `Both(k)` |
| `Retryable` | No | request sent once | Opt this request into the Retry layer (transient errors + 5xx) |
| `BufferMode` | No | `Stream` | `Buffer` collects the whole body inside the retry/breaker boundary |

> **Why fail-closed?** A missing `RateScope` is a bug, not "no limit" — silently
> unthrottled traffic can breach a venue's rate limits and trip a self-inflicted
> outage. Use `RateScope::None` to *explicitly* opt out of pacing.

See `examples/client_with_directives.rs` for a runnable end-to-end example.
```

- [ ] **Step 3: Build + run the example, render docs**

Run: `cargo build -p oath-adapter-net-http-hyper --examples && cargo run -p oath-adapter-net-http-hyper --example client_with_directives`
Expected: prints `venue said: pong`, exits 0.

> **Implementer note:** confirm `hyper`, `hyper-util`, `http-body-util`, `tokio` (with `macros`,`rt-multi-thread`,`net`) are dev-deps of the hyper crate so the example compiles — the inventory confirms `tokio` (macros/rt/net/io-util/test-util), `hyper` (server), `http-body-util` are present. Add `rt-multi-thread` to the tokio dev-dep features if `#[tokio::main]` fails to resolve a runtime.

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/net/http/hyper/examples crates/adapter/net/http/hyper/README.md
git commit -m "docs(net): worked example + README for the per-request extension protocol"
```

---

### Task 15: Record the loom-defer decision (documented defer)

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (a `//` comment near the `Arc<Mutex<Breaker>>`) and `crates/adapter/net/http/api/src/rate_limit.rs` (near the token-bucket `Mutex`)
- Modify: `docs/superpowers/plans/2026-07-06-net-http-deep-review.md` (§4 defer log) **or** append to issue #101's defer notes

**Decision (confirmed):** defer loom — the shared mutexes are held only for tiny critical sections and never across an `await`, so an interleaving test buys little now.

- [ ] **Step 1: Add a code comment** at each shared-lock site, e.g. above the breaker's `Arc<Mutex<Breaker>>` field/usage:

```rust
    // Concurrency-test note (loom): the breaker mutex is held only for the small
    // admit()/record() critical sections and is NEVER held across an `.await`, so a
    // loom interleaving model adds little over the clock-injected unit tests. Deferred
    // deliberately (Tier-1 PR8); revisit if the lock scope ever spans an await or the
    // contention model changes.
```

and the analogous comment at the token-bucket `Mutex<…>` in `rate_limit.rs`.

- [ ] **Step 2: Record the defer in the deep-review §4 log** — append one bullet under the test-strategy section:

```markdown
- **loom deferred (PR8/#101, 2026-07-08):** no loom model for `Mutex<TokenState>` /
  `Arc<Mutex<Breaker>>`. Both locks are held only across tiny non-`await` critical
  sections; a loom test adds little now. Revisit if a lock scope grows to span an
  `await`. Tracked as a Tier-2 candidate.
```

- [ ] **Step 3: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/rate_limit.rs docs/superpowers/plans/2026-07-06-net-http-deep-review.md
git commit -m "docs(net): record the loom-defer decision for the shared breaker/bucket mutexes"
```

---

## Group D — Wrap-up

### Task 16: CHANGELOG entry + full CI gate

**Files:**
- Modify: `CHANGELOG.md` (`[Unreleased]` → `### Changed` or a new `### Added`/`### Fixed` as appropriate — this PR is test/docs, so `### Changed` with a "test/docs" note, matching the repo's style)
- Optional cleanup: delete the stale untracked `docs/superpowers/plans/2026-07-05-net-http-hyper-backend-pr-b.md` (the buffering work it planned merged as #92) — confirm with the user first; it's untracked, so a separate housekeeping commit or leave it.

- [ ] **Step 1: Add the CHANGELOG entry** under `## [Unreleased]`:

```markdown
- **net-http:** closed the Tier-1 resilience **test debt** (M10) and documentation
  gaps (issue #101). Added regression tests for the rate-limiter wait+refill park loop
  (`max_wait > 0`), exact refill rate, `RateScope::Both` acquire order, RateLimit-
  outside-Timeout permit-wait, no burst over-admission, the Half-Open + 429 re-trip on
  `throttle_cooldown`, the Retry backoff doubling ladder, and a SplitMix64 golden
  vector; integration tests exercising the assembled `stack()` over the **real** hyper
  leaf (reset→retry, 429→breaker-trip, send-timeout) plus a positive HTTP/2-keepalive
  survival test. Added doctests for `stack`/`build`/`HttpClient`/`RateScope`/the layer
  factories, a worked `examples/` + README for the mandatory per-request extension
  protocol, and fixed stale rustdoc (L7/L8) and tautological rate-config tests (L12).
  Test/docs only — no behaviour or public-API change. The loom concurrency model is
  deliberately deferred (documented).
```

- [ ] **Step 2: Run the full local gate**

Run: `just ci`
Expected: **PASS** (fmt, lint, test incl. doctests, doc, deny, typos). Then:

Run: `just msrv`
Expected: **PASS** on Rust 1.90.

> **If `just ci` fails:** it is the source of truth (identical to GitHub Actions). Do not bypass hooks. Fix forward — clippy `all` is deny-level, so any new warning blocks. New test code is exempt from the `unwrap`/`expect` lints; production code (only touched for doctests/rustdoc here) is not.

- [ ] **Step 3: Commit + push + PR**

```bash
git add CHANGELOG.md
git commit -m "docs(net): changelog for Tier-1 test-debt + docs (closes #101)"
git push -u origin test/net-http-test-debt
gh pr create --title "test(net): close Tier-1 resilience test gaps + docs/examples (#101)" \
  --body "$(cat <<'EOF'
Closes #101. Final Tier-1 remediation PR (PR8) for the net-http stack.

Test/docs debt only — every behaviour asserted here was fixed and shipped in #104–#113;
this PR adds the regression guards, integration coverage over the real hyper leaf,
doctests, a worked example + README for the per-request extension protocol, and fixes
stale rustdoc + tautological tests. loom is deferred (documented); the h2-keepalive
reaping negative case is deferred (documented).

See docs/superpowers/plans/2026-07-08-net-http-test-debt-pr8.md.
EOF
)"
```

Expected: PR opens green once Cloud CI + MSRV pass.

---

## Self-Review (completed by plan author)

**Spec coverage (PR8, spec lines 144-159):**
- M10 wait+refill (`max_wait>0`) → Task 1 ✓; refill-rate → Task 2 ✓; RateLimit-outside-Timeout → Task 5 ✓; Half-Open+`TripNow` re-trip → Task 6 ✓ (confirmed *not* covered by #104); `Scope::Both` order → Task 3 ✓; retry backoff pinning → Task 7 ✓.
- Integration over the real leaf → Task 9 ✓; the two reasoned-not-observed findings: burst over-admission → Task 4 ✓, h2-keepalive → Task 10 ✓ (positive; reaping deferred per decision).
- SplitMix64 golden vector → Task 8 ✓; refill-**rate** assertion → Task 2 ✓; loom → Task 15 (documented defer per decision) ✓.
- L12 → Task 11 ✓; L13 doctests → Task 13 ✓; L7/L8 → Task 12 ✓.
- `examples/` + README → Task 14 ✓. `just doc` clean → Tasks 12/13/16 ✓. `just ci` green → Task 16 ✓.

**Already-covered (correctly excluded, per inventory):** C1 regression test (`only_a_429_response_trips_now_not_a_local_throttled_error`), M1 probe-guard, L3 jitter decorrelation, M4 buffered-permit-release, `spawn_local` removal — all present at HEAD; no tasks duplicate them.

**Placeholder scan:** every code step contains complete, paste-ready code drawn from the actual test helpers (`Leaf`, `StubBody`, `ScriptLeaf`, `http_cfg`, `rate_cfg`, `config`/`layer`/`req`, `MockTimer`, `cfg`/`secs`/`bare_req`, `backoff_ceiling`, `SplitMix64`, `spawn_echo`). Two intentionally-flagged judgment points (the `stack()` doctest body; the h2 `#[ignore]` fallback) carry explicit implementer guidance, not silent TODOs.

**Type/name consistency:** helper signatures verified against source — `layer(MockTimer, Duration)`, `RateLimitLayer::new(&cfg, timer, max_wait)`, `stack(leaf, cfg, timer, NoAuth, rates)`, `build(cfg, TokioTimer, NoAuth, rates, conn)`, `CircuitBreakerLayer::new(cfg(threshold, cooldown, throttle, probes), timer)`, `backoff_ceiling(base, cap, attempt)`, `SplitMix64::{new, next_u64}`. `HttpError` has no `PartialEq` (assert via `matches!`), and the opaque `stack()`/`build()` `Ok` type has no `Debug` (extract errors via `let…else`) — both reflected throughout.
