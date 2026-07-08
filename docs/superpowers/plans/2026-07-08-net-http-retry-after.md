# net-http `Retry-After` honoring (429 / 5xx) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honor a `delay-seconds` `Retry-After` response header at two disjoint sites — as the `5xx` retry backoff floor and as the `429` circuit-breaker reopen deadline — bounded, panic-free, falling back to existing behavior when absent/unparsable.

**Architecture:** A new zero-dependency `crate::retry_after` parser (`delay-seconds` only). The `Retry` layer reads it on a retryable `5xx` and computes `min(cap, max(honored, jittered))`. The `CircuitBreaker` reads it on a `429` response and sets the reopen deadline to `min(honored, retry_after_cap)`, falling back to `retry_after_fallback`. The two sites are disjoint (a `429` is never retried, a `5xx` is never paced by the breaker), so no response is paced twice. `Breaker::record` gains an `Option<Duration>`; `CircuitBreakerConfig`'s `throttle_cooldown` is renamed `retry_after_fallback` and gains a sibling `retry_after_cap`.

**Tech Stack:** Rust 2024, `oath-adapter-net-http-api` (the resilience layers live here), `http`/`bytes`, `oath-adapter-net-mock::MockTimer` (virtual clock), the `metrics` facade. **No new dependency.**

**Spec:** [docs/superpowers/specs/2026-07-08-net-http-retry-after-design.md](../specs/2026-07-08-net-http-retry-after-design.md).

## Global Constraints

- **Edition 2024, MSRV 1.90.** Validate with `just msrv` (final task).
- **No `unsafe`** (`unsafe_code = "deny"`); **no `unwrap`/`expect`/indexing in non-test code** (clippy `all` deny-level) — return `Result`/`Option`, model errors with `thiserror`. **Test code is exempt** (new tests may `unwrap`/`expect`, matching existing test modules).
- **`missing_docs` warned** — every new `pub` item gets a `///`. (This feature adds no new `pub` items except two config fields, which need docs.)
- **Definition of done = `just ci` passes** (fmt, lint, test, doc, deny, typos). Per the net-http rule, run **`just doc`** in each task's checks — `check`/`lint`/`test` miss broken rustdoc intra-doc links.
- **Conventional Commits**, enforced by the `commit-msg` hook; subject ≤ 72 chars. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Worktree:** all work in `.claude/worktrees/net-http-retry-after` on branch `feat/net-http-retry-after`, created **off `main`**. Never touch the primary checkout's branch.
- **CHANGELOG:** one `[Unreleased]` entry (final task).

---

## Setup (once, before Task 1)

Create the isolated worktree off the latest `main` (the primary checkout may be behind):

```bash
git -C /workspaces/oath fetch origin
git -C /workspaces/oath worktree add -b feat/net-http-retry-after \
  /workspaces/oath/.claude/worktrees/net-http-retry-after origin/main
```

All paths below are repo-root-relative; edit them **inside the worktree**
(`.claude/worktrees/net-http-retry-after/…`) and run every `just`/`cargo` command with
that worktree as the working directory.

**Baseline** — confirm green before adding anything:

```bash
just check && just test
```
Expected: PASS (HEAD is `origin/main`, all green).

> **Note — line numbers are approximate.** This plan was written against `main` at #113;
> `main` has since advanced through #114 (Tier-1 test debt) and #115 (docs). Test modules
> grew, so **locate every edit by the shown content, not by line number**. All the code
> anchors this plan edits still exist verbatim; only their positions moved.

---

### Task 1: The `Retry-After` parser module

**Files:**
- Create: `crates/adapter/net/http/api/src/retry_after.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs` (add `mod retry_after;`)

**Interfaces:**
- Consumes: `http::HeaderMap`, `http::header::RETRY_AFTER` (already available deps).
- Produces: `pub(crate) fn parse_retry_after(headers: &http::HeaderMap) -> Option<Duration>` — consumed by Task 4 (`Retry`) and Task 5 (`CircuitBreaker`).

- [ ] **Step 1: Create the module with the parser and its tests**

Create `crates/adapter/net/http/api/src/retry_after.rs`:

```rust
//! Parse the `Retry-After` response header (RFC 9110 §10.2.3), `delay-seconds` form.
//!
//! `Retry-After` rides on `429`/`503` responses. The `Retry` and `CircuitBreaker`
//! layers read it (read-only, ADR-0034 §4) to pace by the venue's directive instead
//! of a purely local schedule (ADR-0031 Amendment #2). Only the `delay-seconds` form
//! is honored; an `HTTP-date`, a float, an overflowing integer, or an absent header
//! yields `None`, and the caller falls back to its own default — `Retry-After` is an
//! untrusted hint, so parsing never errors and never panics.

use std::time::Duration;

/// The venue-directed wait from a `Retry-After` header, `delay-seconds` form only.
///
/// Returns `None` for an absent header, a non-ASCII value, or any non-integer form
/// (an `HTTP-date`, a float such as `1.5`, a negative, or junk) — the caller falls
/// back to its own default. Never panics: `Duration::from_secs` is total over `u64`,
/// and an out-of-`u64` value simply fails to parse. The value is **uncapped** — each
/// caller clamps to its own ceiling.
pub(crate) fn parse_retry_after(headers: &http::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::parse_retry_after;
    use std::time::Duration;

    fn headers_with(value: &str) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        h.insert(
            http::header::RETRY_AFTER,
            http::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn parses_the_delay_seconds_form() {
        assert_eq!(
            parse_retry_after(&headers_with("120")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after(&headers_with("0")),
            Some(Duration::ZERO),
            "0 is a valid 'retry now'"
        );
        assert_eq!(
            parse_retry_after(&headers_with("  120  ")),
            Some(Duration::from_secs(120)),
            "surrounding whitespace is trimmed"
        );
        assert_eq!(
            parse_retry_after(&headers_with("259200")),
            Some(Duration::from_secs(259_200)),
            "a large valid integer parses; the CALLER caps it, not the parser"
        );
    }

    #[test]
    fn an_absent_header_is_none() {
        assert_eq!(parse_retry_after(&http::HeaderMap::new()), None);
    }

    #[test]
    fn non_integer_forms_fall_back_to_none() {
        // The HTTP-date form (deferred — needs a wall-clock Timer seam), a float, a
        // negative, and junk all yield None so the caller keeps its own schedule.
        assert_eq!(
            parse_retry_after(&headers_with("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(&headers_with("1.5")), None);
        assert_eq!(parse_retry_after(&headers_with("-5")), None);
        assert_eq!(parse_retry_after(&headers_with("soon")), None);
    }

    #[test]
    fn an_overflowing_integer_is_none_not_a_panic() {
        // u64::MAX + 1 — must not panic, just fail to parse (the no-panic guarantee).
        assert_eq!(
            parse_retry_after(&headers_with("18446744073709551616")),
            None
        );
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/adapter/net/http/api/src/lib.rs`, add a private `mod retry_after;` next to
the other private module (`mod clock;`, line 37). Insert immediately after that line:

```rust
mod clock;
mod retry_after;
```

- [ ] **Step 3: Run the parser tests to verify they pass**

Run: `cargo test -p oath-adapter-net-http-api retry_after::`
Expected: PASS (5 tests). **Guard mutation:** parsing `"1.5"`/`"-5"`/an HTTP-date as
anything but `None`, or panicking on the overflow input, fails these tests.

- [ ] **Step 4: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS — no new warnings; the module rustdoc has no broken intra-doc links.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/retry_after.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): Retry-After delay-seconds parser (zero-dep)"
```

---

### Task 2: `CircuitBreakerConfig` — rename `throttle_cooldown` + add `retry_after_cap`

Rename the field whose sole use is the `429` reopen (`throttle_cooldown` →
`retry_after_fallback`) and add the honored-value ceiling (`retry_after_cap`). No
behavior change yet — the two `TripNow` arms just use the renamed field; honoring lands
in Task 5. The one **new** behavior is the boot-time `retry_after_cap == 0` rejection.

> **⚠️ Scope note — `main` advanced past #114/#115 after this plan was written.**
> The line numbers below are approximate; **locate every edit by content and drive the
> rename with `grep -rn "throttle_cooldown" crates/`.** #114 added **doctests** and an
> **example** that also construct `CircuitBreakerConfig`, so the rename scope is larger
> than the four sites first listed. There are now **9 `CircuitBreakerConfig { … }`
> literals** — each needs its `throttle_cooldown:` key renamed to `retry_after_fallback:`
> **and** a new `retry_after_cap: Duration::from_secs(1800),` line added:
> 1. `circuit_breaker.rs` — `mod breaker_tests` `cfg` helper
> 2. `circuit_breaker.rs` — `mod service_tests` `cfg` helper
> 3. `circuit_breaker.rs` — the `CircuitBreakerLayer::new` **doctest** (`/// … throttle_cooldown: …`)
> 4. `stack.rs` — the `http_cfg` test helper
> 5. `stack.rs` — the `HttpConfig`/`stack` **doctest**
> 6. `build.rs` — the `build()` **doctest** (a single inline `/// … CircuitBreakerConfig { … } …` line)
> 7. `build.rs` — the test-config construction
> 8. `examples/client_with_directives.rs` — the example's config literal
> 9. (the struct definition itself — the field decl, Step 1 below)
>
> Plus the **name-only references** (rename, no field add): the two `TripNow` arms, the
> module doc, `validate_config`'s check + `"circuit_breaker.throttle_cooldown"` error
> string, `rate.rs`'s doc comment, and the zero-duration test's expected error string.
> Plus **stale comments/strings** mentioning `throttle_cooldown` in the #114 Half-Open
> tests and `build.rs` — update their wording to `retry_after_fallback`. **A doctest or
> example left with the old field name will fail `just doc`/`just ci`.** Confirm zero
> remaining hits: `grep -rn "throttle_cooldown" crates/` returns nothing when done.

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (struct, module doc, both `TripNow` arms, both test `cfg` helpers, the `CircuitBreakerLayer::new` doctest, stale test comments)
- Modify: `crates/adapter/net/http/api/src/stack.rs` (`validate_config` + its doc, the `HttpConfig`/`stack` doctest, the test `http_cfg` helper, the zero-duration test)
- Modify: `crates/adapter/net/http/hyper/examples/client_with_directives.rs` (the example config literal)
- Modify: `crates/adapter/net/http/api/src/rate.rs` (one doc-comment name reference)
- Modify: `crates/adapter/net/http/hyper/src/build.rs` (the `build()` **doctest** inline config + the test-config construction + a comment)

**Interfaces:**
- Produces: `CircuitBreakerConfig { failure_threshold, cooldown, retry_after_fallback: Duration, retry_after_cap: Duration, half_open_probes }` — consumed by Task 5 and by every config-construction site.

- [ ] **Step 1: Rename the field and add the cap in the struct**

In `crates/adapter/net/http/api/src/circuit_breaker.rs`, replace the
`throttle_cooldown` field (line 47) — i.e. change:

```rust
    /// The (longer) cooldown after a `Throttled`/429 trip — the penalty box.
    pub throttle_cooldown: Duration,
```

to:

```rust
    /// The `429` reopen wait when the response carries **no** usable `Retry-After`
    /// (the penalty-box fallback; ≈ 10–15 min for IBKR). Renamed from
    /// `throttle_cooldown` (ADR-0031 Amendment #2).
    pub retry_after_fallback: Duration,
    /// Ceiling on an **honored** `429` `Retry-After`: `reopen = min(retry_after, cap)`.
    /// May be set `≥ retry_after_fallback` to honor a directive *longer* than the
    /// default box; also bounds a hostile/absurd `Retry-After` (ADR-0031 Amendment #2).
    pub retry_after_cap: Duration,
```

- [ ] **Step 2: Update the module doc and both `TripNow` reopen sites**

In the same file, module doc line 8 — change `[`CircuitBreakerConfig::throttle_cooldown`]`
to `[`CircuitBreakerConfig::retry_after_fallback`]`.

Then both `TripNow` arms (lines 236 and 257) currently read
`reopen_at: deadline(now, self.cfg.throttle_cooldown),`. Change **both** to:

```rust
                        reopen_at: deadline(now, self.cfg.retry_after_fallback),
```

- [ ] **Step 3: Update the two in-file test `cfg` helpers**

`mod breaker_tests` helper (line 561) — change to:

```rust
    fn cfg(threshold: u32, probes: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown: Duration::from_secs(30),
            retry_after_fallback: Duration::from_secs(900),
            retry_after_cap: Duration::from_secs(1800),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }
```

`mod service_tests` helper (line 878) — rename the field and add the cap (keep the
positional signature so existing call sites are unchanged):

```rust
    fn cfg(
        threshold: u32,
        cooldown: Duration,
        fallback: Duration,
        probes: u32,
    ) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: NonZeroU32::new(threshold).unwrap(),
            cooldown,
            retry_after_fallback: fallback,
            retry_after_cap: Duration::from_secs(1800),
            half_open_probes: NonZeroU32::new(probes).unwrap(),
        }
    }
```

- [ ] **Step 4: Update `stack.rs` — construction site, doc, `validate_config`**

In `crates/adapter/net/http/api/src/stack.rs`:

(a) The test `http_cfg` `CircuitBreakerConfig` (lines 310-315) — change to:

```rust
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: NonZeroU32::new(3).unwrap(),
                cooldown: Duration::from_secs(30),
                retry_after_fallback: Duration::from_secs(900),
                retry_after_cap: Duration::from_secs(1800),
                half_open_probes: NonZeroU32::new(1).unwrap(),
            },
```

(b) `validate_config`'s doc (line 111) — replace `` `cooldown`/`throttle_cooldown == 0` ``
with `` `cooldown`/`retry_after_fallback`/`retry_after_cap == 0` ``.

(c) `validate_config` body (lines 122-128) — replace the `throttle_cooldown` check and
add the cap check:

```rust
    if cfg.circuit_breaker.cooldown.is_zero() {
        return Err(BuildError::ZeroDuration("circuit_breaker.cooldown"));
    }
    if cfg.circuit_breaker.retry_after_fallback.is_zero() {
        return Err(BuildError::ZeroDuration("circuit_breaker.retry_after_fallback"));
    }
    if cfg.circuit_breaker.retry_after_cap.is_zero() {
        return Err(BuildError::ZeroDuration("circuit_breaker.retry_after_cap"));
    }
```

- [ ] **Step 5: Update `rate.rs` doc reference and `hyper/build.rs` config**

In `crates/adapter/net/http/api/src/rate.rs` line 148, change the doc-comment text
`` `cooldown`/`throttle_cooldown == 0` `` to
`` `cooldown`/`retry_after_fallback == 0` ``.

In `crates/adapter/net/http/hyper/src/build.rs` there are **two** config literals plus a
comment (grep confirms). For the **test-config** construction, replace the
`throttle_cooldown: Duration::from_secs(900),` line with:

```rust
                retry_after_fallback: Duration::from_secs(900),
                retry_after_cap: Duration::from_secs(1800),
```

For the **`build()` doctest** — a single inline line
`/// … cooldown: Duration::from_secs(30), throttle_cooldown: Duration::from_secs(900), half_open_probes: …` —
rename the key and insert the cap inline:
`… cooldown: Duration::from_secs(30), retry_after_fallback: Duration::from_secs(900), retry_after_cap: Duration::from_secs(1800), half_open_probes: …`.
Also update the `// … throttle_cooldown …` comment wording to `retry_after_fallback`.

Likewise in `crates/adapter/net/http/hyper/examples/client_with_directives.rs`, replace
its `throttle_cooldown: Duration::from_secs(900),` line with the same two lines
(`retry_after_fallback` + `retry_after_cap`) shown above for the test-config.

- [ ] **Step 6: Update the existing zero-duration test + add the cap test**

In `crates/adapter/net/http/api/src/stack.rs`, the existing test (lines ~421-430)
asserts `throttle_cooldown == 0` is a `BuildError`. Replace its field write and
expected error:

```rust
        // retry_after_fallback == 0 would collapse the 429 penalty box.
        cfg.circuit_breaker.retry_after_fallback = Duration::ZERO;
        let Err(err) = stack(leaf.clone(), cfg.clone(), MockTimer::new(), NoAuth, rate_cfg())
        else {
            panic!("retry_after_fallback == 0 must be a BuildError");
        };
        assert_eq!(
            err,
            BuildError::ZeroDuration("circuit_breaker.retry_after_fallback")
        );
```

> **Implementer note:** copy the exact `stack(...)` call shape, `leaf`/`cfg` bindings,
> and the `let Err(...) else` form from the surrounding test — the snippet above shows
> the field write and the expected error; keep the test's existing setup lines.

Then add a sibling test immediately after it, for the new field:

```rust
    #[test]
    fn zero_retry_after_cap_is_a_build_error() {
        // A zero cap would honor every 429 Retry-After as an immediate probe.
        let leaf = MockLeaf;
        let mut cfg = http_cfg(1, Duration::from_secs(1), Duration::ZERO);
        cfg.circuit_breaker.retry_after_cap = Duration::ZERO;
        let Err(err) = stack(leaf, cfg, MockTimer::new(), NoAuth, rate_cfg()) else {
            panic!("retry_after_cap == 0 must be a BuildError");
        };
        assert_eq!(
            err,
            BuildError::ZeroDuration("circuit_breaker.retry_after_cap")
        );
    }
```

> **Implementer note:** mirror the leaf/`http_cfg(...)` arguments the neighbouring
> zero-duration test uses (`MockLeaf`, `http_cfg`, `rate_cfg`, `NoAuth` are all already
> in scope in `stack.rs`'s test module). Adjust the `http_cfg(...)` args to match its
> real signature if it differs from `(retry_attempts, timeout, max_wait)`.

- [ ] **Step 7: Build and run the affected suites**

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker:: stack:: && cargo test -p oath-adapter-net-http-hyper`
Expected: PASS — the rename compiles everywhere, existing breaker/stack tests are green
under the new field name, and both zero-duration tests pass. **Guard mutation:** dropping
either `is_zero()` check makes its test admit the config → the `panic!` fires.

- [ ] **Step 8: Verify doctests + example + lint + docs, then commit**

Run: `just test && just lint && just doc`
Expected: PASS — `just test` compiles/runs the **doctests** (the renamed
`CircuitBreakerConfig` literals in `circuit_breaker.rs`/`stack.rs`/`build.rs` doctests),
`just lint` (clippy `--all-targets`) compiles the **example**
(`client_with_directives.rs`), and `just doc` resolves rustdoc links. A single missed
`throttle_cooldown` in any doctest/example fails here. Final gate:
`grep -rn "throttle_cooldown" crates/` returns nothing.

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs \
        crates/adapter/net/http/api/src/stack.rs \
        crates/adapter/net/http/api/src/rate.rs \
        crates/adapter/net/http/hyper/src/build.rs \
        crates/adapter/net/http/hyper/examples/client_with_directives.rs
git commit -m "refactor(net)!: rename throttle_cooldown->retry_after_fallback + add retry_after_cap"
```

---

### Task 3: `retry_after_honored` metric

**Files:**
- Modify: `crates/adapter/net/http/api/src/meter.rs` (const + fn + one test)

**Interfaces:**
- Produces: `pub(crate) fn retry_after_honored(site: &'static str)` — consumed by Task 4 (`"retry"`) and Task 5 (`"breaker"`).

- [ ] **Step 1: Add the counter constant and emit function**

In `crates/adapter/net/http/api/src/meter.rs`, add the constant after `BACKOFF_SECONDS`
(line 48):

```rust
/// Counter: honored `Retry-After` directives, labelled by `site` (`"retry" | "breaker"`).
const RETRY_AFTER_HONORED: &str = "http_retry_after_honored_total";
```

And the emit function after `backoff` (line 81):

```rust
/// Count one honored `Retry-After` directive at `site` (`"retry"` or `"breaker"`).
pub(crate) fn retry_after_honored(site: &'static str) {
    metrics::counter!(RETRY_AFTER_HONORED, "site" => site).increment(1);
}
```

- [ ] **Step 2: Write the metric test**

Add to `meter.rs`'s `#[cfg(test)] mod tests` (mirror `throttled_counter_carries_the_route_label`).
First extend the `use super::{…}` import (line 85) to include `retry_after_honored`,
then add:

```rust
    #[test]
    fn retry_after_honored_carries_the_site_label() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            retry_after_honored("breaker");
        });
        let counter = snap
            .snapshot()
            .into_vec()
            .into_iter()
            .find(|(k, _, _, _)| k.key().name() == "http_retry_after_honored_total")
            .expect("counter emitted");
        assert!(
            counter
                .0
                .key()
                .labels()
                .any(|l| l.key() == "site" && l.value() == "breaker"),
            "labelled site=breaker"
        );
        assert_eq!(counter.3, DebugValue::Counter(1));
    }
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p oath-adapter-net-http-api meter::`
Expected: PASS.

- [ ] **Step 4: Verify lint + docs, then commit**

Run: `just lint && just doc`
Expected: PASS.

```bash
git add crates/adapter/net/http/api/src/meter.rs
git commit -m "feat(net): retry_after_honored metric (site-labelled counter)"
```

---

### Task 4: Site 1 — `Retry` honors `Retry-After` on a retryable 5xx

**Files:**
- Modify: `crates/adapter/net/http/api/src/retry.rs` (the retry-loop backoff computation + the in-file test `Step`/`ScriptLeaf` + two tests)

**Interfaces:**
- Consumes: `crate::retry_after::parse_retry_after` (Task 1); `crate::meter::retry_after_honored` (Task 3); `self.cfg.cap`, `backoff_ceiling`, `self.rng.duration_in` (existing).
- Produces: no new public items — `Retry::call`'s backoff gains the honored floor.

- [ ] **Step 1: Add a `StatusRetryAfter` step to the test leaf**

In `retry.rs`'s `#[cfg(test)] mod tests`, extend the `Step` enum (line 374) — add the
new variant (it stays `Copy`):

```rust
    #[derive(Clone, Copy)]
    enum Step {
        Err(ErrorKind),
        Status(u16),
        StatusRetryAfter(u16, u64),
    }
```

And add its arm to `ScriptLeaf`'s `match step` (after the `Step::Status` arm, ~line 431):

```rust
                    Step::StatusRetryAfter(code, secs) => {
                        let mut resp = http::Response::new(StubBody::new(b"body"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        resp.headers_mut()
                            .insert(http::header::RETRY_AFTER, http::HeaderValue::from(secs));
                        Ok(resp)
                    },
```

- [ ] **Step 2: Write the failing backoff-floor test**

Add to the same `tests` module:

```rust
    #[tokio::test]
    async fn retry_after_on_a_5xx_sets_the_backoff_floor() {
        // base = 0 → jittered ceiling 0 → jittered = 0. A 503 carrying Retry-After: 5
        // must make the retry sleep 5s (the server floor), not 0. cap = 10s ≥ 5s, so
        // the honored value is not clamped. If honoring regressed, the delay would be 0
        // and the retry would fire immediately with no 5s park.
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(503, 5), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, Duration::ZERO, Duration::from_secs(10)), timer.clone())
            .layer(leaf.clone());
        let handle = tokio::spawn(async move { svc.call(req(true)).await });
        tokio::task::yield_now().await; // attempt 1 → 503, parks on the 5s Retry-After sleep
        assert!(
            !handle.is_finished(),
            "an honored Retry-After must park the retry; a 0 backoff would have finished it"
        );
        timer.advance(Duration::from_secs(5)); // wake the parked retry
        let resp = handle.await.unwrap().expect("2nd attempt after the honored wait → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p oath-adapter-net-http-api retry_after_on_a_5xx_sets_the_backoff_floor`
Expected: **FAIL** — today the delay is `duration_in(ceil)` with `ceil = 0`, so the
retry does not park; `!handle.is_finished()` fails (the task already completed after the
first yield).

- [ ] **Step 4: Implement the honored backoff floor**

In `retry.rs`'s `Retry::call`, the retry-continuation block currently reads (lines ~299-305):

```rust
                if !retry {
                    tracing::Span::current().record("attempts", u64::from(attempt));
                    return outcome; // success or a non-retryable verdict
                }
                drop(outcome); // release the prior response's Guarded permit before waiting
                let ceil = backoff_ceiling(self.cfg.base, self.cfg.cap, attempt);
                let delay = self.rng.duration_in(ceil);
```

Replace it with:

```rust
                if !retry {
                    tracing::Span::current().record("attempts", u64::from(attempt));
                    return outcome; // success or a non-retryable verdict
                }
                // Honor a delay-seconds `Retry-After` on the retryable 5xx as a backoff
                // FLOOR (ADR-0031 Amendment #2): read it before the response is dropped.
                let honored = match &outcome {
                    Ok(resp) => crate::retry_after::parse_retry_after(resp.headers()),
                    Err(_) => None,
                };
                drop(outcome); // release the prior response's Guarded permit before waiting
                let jittered = self
                    .rng
                    .duration_in(backoff_ceiling(self.cfg.base, self.cfg.cap, attempt));
                // Server value overrides local backoff (never re-jittered), floored by
                // our own jittered schedule, and capped by `RetryConfig::cap`.
                let delay = honored.map_or(jittered, |ra| ra.min(self.cfg.cap).max(jittered));
                if honored.is_some() {
                    crate::meter::retry_after_honored("retry");
                }
```

- [ ] **Step 5: Run it to verify it passes**

Run: `cargo test -p oath-adapter-net-http-api retry_after_on_a_5xx_sets_the_backoff_floor`
Expected: **PASS**.

- [ ] **Step 6: Add the cap-clamp test**

```rust
    #[tokio::test]
    async fn retry_after_on_a_5xx_is_clamped_to_the_retry_cap() {
        // Retry-After: 100 but cap = 2s → the honored wait clamps to 2s, not 100s.
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(503, 100), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, Duration::ZERO, Duration::from_secs(2)), timer.clone())
            .layer(leaf.clone());
        let handle = tokio::spawn(async move { svc.call(req(true)).await });
        tokio::task::yield_now().await; // parks on the clamped 2s sleep
        timer.advance(Duration::from_secs(2)); // the 2s clamp elapses; a 100s sleep would not
        tokio::task::yield_now().await; // let the woken retry run attempt 2
        assert!(
            handle.is_finished(),
            "the honored value must be clamped to cap (2s), not held for 100s"
        );
        let resp = handle.await.unwrap().expect("clamped wait elapsed → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
    }
```

Run: `cargo test -p oath-adapter-net-http-api retry_after_on_a_5xx_is_clamped_to_the_retry_cap`
Expected: **PASS**. **Guard mutation:** dropping the `.min(self.cfg.cap)` clamp makes the
delay 100s, so after a 2s advance the task is still parked → `handle.is_finished()` is
false → the assert fails (cleanly, no hang).

> **Note — absent-header regression is already covered:** `eligible_5xx_is_retried`
> (a 503 with no `Retry-After`) still exercises the `honored == None → jittered` path and
> must stay green; do not delete it.

- [ ] **Step 7: Run the full retry suite + lint + docs**

Run: `cargo test -p oath-adapter-net-http-api retry:: && just lint && just doc`
Expected: PASS — the two new tests plus every existing `retry::` test green.

- [ ] **Step 8: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs
git commit -m "feat(net): Retry honors Retry-After as a 5xx backoff floor"
```

---

### Task 5: Site 2 — `CircuitBreaker` honors `Retry-After` on a 429 reopen

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (`Breaker::record` signature + both `TripNow` arms; the service `call` extraction; every `record` call in `mod breaker_tests`; the `service_tests` `Step`/`ScriptLeaf`; new tests in both test modules)

**Interfaces:**
- Consumes: `crate::retry_after::parse_retry_after` (Task 1); `crate::meter::retry_after_honored` (Task 3); `self.cfg.retry_after_fallback`, `self.cfg.retry_after_cap` (Task 2).
- Produces: `Breaker::record(&mut self, class: Class, now: Instant, retry_after: Option<Duration>)`.

- [ ] **Step 1: Write the failing pure-core tests** (in `mod breaker_tests`)

Add these three tests. They call `record` with the **new** third argument, so they will
not compile until Step 3 — that is the intended red state.

```rust
    #[test]
    fn a_429_retry_after_reopens_on_the_honored_value() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // trips on the first outcome
        // 429 carrying Retry-After: 2 → reopen at now+2s (honored), under the 900s
        // fallback and the 1800s cap.
        b.record(Class::TripNow, now, Some(Duration::from_secs(2)));
        assert_eq!(
            b.admit(now + Duration::from_secs(1)),
            Admit::Reject,
            "before the honored 2s"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(2)),
            Admit::Probe,
            "honored 2s elapsed → probe"
        );
    }

    #[test]
    fn a_429_retry_after_is_clamped_to_the_cap() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // retry_after_cap = 1800s
        b.record(Class::TripNow, now, Some(Duration::from_secs(100_000))); // absurd
        assert_eq!(
            b.admit(now + Duration::from_secs(1799)),
            Admit::Reject,
            "before the 1800s cap"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(1800)),
            Admit::Probe,
            "clamped to the 1800s cap, not 100_000s"
        );
    }

    #[test]
    fn a_429_without_retry_after_uses_the_fallback() {
        let now = Instant::now();
        let mut b = Breaker::new(cfg(1, 1)); // retry_after_fallback = 900s
        b.record(Class::TripNow, now, None);
        assert_eq!(
            b.admit(now + Duration::from_secs(899)),
            Admit::Reject,
            "before the 900s fallback"
        );
        assert_eq!(
            b.admit(now + Duration::from_secs(900)),
            Admit::Probe,
            "fallback 900s elapsed → probe"
        );
    }
```

- [ ] **Step 2: Run to verify it fails (does not compile)**

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker:: 2>&1 | head -30`
Expected: **FAIL** — `record` takes 2 args, so the new tests (and the service) don't
compile until Step 3. This confirms the tests target the new signature.

- [ ] **Step 3: Add the `Option<Duration>` parameter to `record` and clamp in both `TripNow` arms**

In `circuit_breaker.rs`, change `Breaker::record`'s signature (line 217):

```rust
    pub(crate) fn record(&mut self, class: Class, now: Instant, retry_after: Option<Duration>) {
```

In the **Closed** state's `Class::TripNow` arm (lines 234-238), replace:

```rust
                Class::TripNow => {
                    self.state = BreakerState::Open {
                        reopen_at: deadline(now, self.cfg.retry_after_fallback),
                    };
                },
```

with:

```rust
                Class::TripNow => {
                    let cooldown = retry_after
                        .map_or(self.cfg.retry_after_fallback, |ra| ra.min(self.cfg.retry_after_cap));
                    self.state = BreakerState::Open {
                        reopen_at: deadline(now, cooldown),
                    };
                },
```

Apply the **identical** change to the **HalfOpen** state's `Class::TripNow` arm (lines
255-259) — same `let cooldown = …; reopen_at: deadline(now, cooldown)` body.

- [ ] **Step 4: Thread the honored value in from the service `call`**

In the `CircuitBreaker` `Service::call` (the record block, lines 480-495), replace:

```rust
            let class = classify(&outcome);
            let transition = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = breaker.phase();
                breaker.record(class, now);
                transition_label(before, breaker.phase())
            };
            if let Some(to) = transition {
                crate::meter::breaker_transition(to);
            }
            outcome
```

with:

```rust
            let class = classify(&outcome);
            // Honor a delay-seconds `Retry-After` only on a 429 response (ADR-0031
            // Amendment #2): it sets the reopen deadline, clamped by `retry_after_cap`.
            let retry_after = match &outcome {
                Ok(resp) if resp.status() == http::StatusCode::TOO_MANY_REQUESTS => {
                    crate::retry_after::parse_retry_after(resp.headers())
                },
                _ => None,
            };
            let transition = {
                let now = self.timer.now();
                let mut breaker = self
                    .breaker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = breaker.phase();
                breaker.record(class, now, retry_after);
                transition_label(before, breaker.phase())
            };
            if let Some(to) = transition {
                crate::meter::breaker_transition(to);
            }
            if retry_after.is_some() {
                crate::meter::retry_after_honored("breaker");
            }
            outcome
```

- [ ] **Step 5: Update every existing `record` call in `mod breaker_tests` to pass `None`**

The pure-core unit tests in `mod breaker_tests` call `b.record(<class>, <now>)`. Add a
third argument `None` to **every** such call (only the new tests from Step 1 pass
`Some`). For example, `b.record(Class::Failure, now)` becomes
`b.record(Class::Failure, now, None)`; `b.record(Class::TripNow, probe_at)` becomes
`b.record(Class::TripNow, probe_at, None)`. Sweep the whole module:

```bash
# From the worktree root — list every call site to update by hand:
grep -n "\.record(" crates/adapter/net/http/api/src/circuit_breaker.rs
```

Update each `mod breaker_tests` call to end with `, None)` (the service call in Step 4
already passes `retry_after`; the three new tests already pass `Some`/`None`).

- [ ] **Step 6: Run the pure-core tests to verify they pass**

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker::breaker_tests`
Expected: **PASS** — the three honoring tests plus every existing breaker unit test.
**Guard mutation:** using `retry_after_fallback` instead of the clamp in a `TripNow` arm
makes `a_429_retry_after_reopens_on_the_honored_value` admit a probe only at +900s → its
+2s `Probe` assertion fails; dropping `.min(retry_after_cap)` makes the clamp test wait
100_000s → its +1800s `Probe` assertion fails.

- [ ] **Step 7: Add the `StatusRetryAfter` step + service-level honoring test**

In `mod service_tests`, extend the `Step` enum (line ~826) to add `StatusRetryAfter(u16, u64)`:

```rust
    #[derive(Clone, Copy)]
    enum Step {
        Err(ErrorKind),
        Status(u16),
        StatusRetryAfter(u16, u64),
    }
```

Add its arm to that module's `ScriptLeaf` `match step` (after the `Step::Status` arm,
~line 872) — note the body type here is `()`:

```rust
                    Step::StatusRetryAfter(code, secs) => {
                        let mut resp = http::Response::new(());
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        resp.headers_mut()
                            .insert(http::header::RETRY_AFTER, http::HeaderValue::from(secs));
                        Ok(resp)
                    },
```

Then add the service test (drives the full breaker service over the real header path):

```rust
    #[tokio::test]
    async fn a_429_response_retry_after_reopens_on_the_honored_value() {
        let timer = MockTimer::new();
        // A single 429 carrying Retry-After: 5 trips the breaker; it must reopen at +5s
        // (honored), NOT the 900s fallback. A probe at +5s reaches the leaf → 200.
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(429, 5), Step::Status(200)]);
        let svc =
            CircuitBreakerLayer::new(cfg(2, secs(30), secs(900), 1), timer.clone()).layer(leaf);
        let resp = svc.call(bare_req()).await.expect("429 returns as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        // +4s is short of the honored 5s → still Open, fast-reject (leaf untouched).
        timer.advance(secs(4));
        assert!(
            matches!(svc.call(bare_req()).await.unwrap_err(), HttpError::CircuitOpen),
            "before the honored 5s the breaker still rejects"
        );
        // +1s more (total 5s) → probe admitted → reaches the leaf → 200.
        timer.advance(secs(1));
        let ok = svc.call(bare_req()).await.expect("honored 5s elapsed → probe → 200");
        assert_eq!(ok.status(), http::StatusCode::OK);
    }
```

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker::service_tests::a_429_response_retry_after_reopens_on_the_honored_value`
Expected: **PASS**. **Guard mutation:** if the service ignored the header (passed `None`),
the reopen would be 900s → the `+5s → 200` `expect` panics with `CircuitOpen`.

- [ ] **Step 8: Run the full breaker suite + lint + docs**

Run: `cargo test -p oath-adapter-net-http-api circuit_breaker:: && just lint && just doc`
Expected: PASS — every breaker test green.

- [ ] **Step 9: Commit**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs
git commit -m "feat(net): CircuitBreaker honors 429 Retry-After for the reopen deadline"
```

---

### Task 6: ADR amendments, CHANGELOG, full CI, issue + PR

**Files:**
- Modify: `docs/adr/0031-http-resilience-venue-pacing.md` (append Amendment #2)
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md` (append Amendment #12)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Append ADR-0031 Amendment #2**

At the end of `docs/adr/0031-http-resilience-venue-pacing.md` (after the Amendment #1
block), add:

```markdown

2. **`Retry-After` honoring (delay-seconds) at two disjoint sites; `throttle_cooldown`
   renamed.** The stack now reads a `delay-seconds` `Retry-After` response header (RFC
   9110 §10.2.3) at two **disjoint** sites — no response is paced twice:
   - **`Retry` (5xx):** on a retryable `5xx` carrying `Retry-After`, the backoff is
     `min(RetryConfig::cap, max(retry_after, jittered))` — the server value is a floor,
     the existing jittered exponential the other floor, capped by `cap`, and the server
     value is **not** re-jittered (the server already jittered).
   - **`CircuitBreaker` (429):** a `429` `TripNow` reopens at
     `min(retry_after, retry_after_cap)` when the header is present, else at the
     `retry_after_fallback` default.
   The §5 config field `throttle_cooldown` is **renamed `retry_after_fallback`** (its
   sole role was this `429` default), and a new **`retry_after_cap`** bounds an honored
   value — it may be set `≥ retry_after_fallback`, so a venue directing a *longer* legit
   ban is honored (up to the cap) rather than probed early into it. `429` is **still
   never retried** (§2 unchanged) — honoring only refines *existing* layer pacing.
   Parsing is fallible and side-effect-free: an `HTTP-date`, a float, an overflowing
   integer, or an absent header is treated as absent (falls back to existing behavior).
   The `HTTP-date` form and alternate/absolute headers stay deferred — they need a
   wall-clock `Timer` seam (ADR-0029). Lands the `delay-seconds` half of ADR-0034
   Amendment #8's deferred "`Retry-After` parsing". Spec:
   `docs/superpowers/specs/2026-07-08-net-http-retry-after-design.md`.
```

- [ ] **Step 2: Append ADR-0034 Amendment #12**

At the end of the amendments list in
`docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md` (after item 11),
add:

```markdown
12. **`Retry-After` honoring (delay-seconds) landed; `throttle_cooldown` renamed.**
    Amendment #8 deferred "`Retry-After` parsing"; its `delay-seconds` half now ships —
    the `Retry` layer honors it as a `5xx` backoff floor and the `CircuitBreaker` uses
    it for the `429` reopen deadline (details + rationale in **ADR-0031 Amendment #2**).
    The breaker's `throttle_cooldown` field is renamed **`retry_after_fallback`**, and a
    new **`retry_after_cap`** bounds an honored value. The `HTTP-date` form and
    alternate/absolute headers stay deferred (they need a wall-clock `Timer` seam).
```

- [ ] **Step 3: Add the CHANGELOG entry**

In `CHANGELOG.md` under `## [Unreleased]`, add an `### Added` bullet (create the
`### Added` subsection if absent; otherwise append to it) and note the breaking rename
under `### Changed`:

```markdown
### Added

- **net-http:** the resilience stack now honors a `delay-seconds` `Retry-After`
  response header at two disjoint sites — as the `5xx` retry backoff floor
  (`min(cap, max(retry_after, jittered))`, un-jittered) and as the `429`
  circuit-breaker reopen deadline (`min(retry_after, retry_after_cap)`, else the
  `retry_after_fallback` default). `429` is still never retried. An `HTTP-date`,
  float, overflowing, or absent value falls back to existing behavior. A new
  site-labelled `http_retry_after_honored_total` metric. (ADR-0031 Amendment #2)
```

Append to `### Changed` (the section already exists):

```markdown
- **Breaking (pre-release) — net-http.** `CircuitBreakerConfig::throttle_cooldown` is
  renamed `retry_after_fallback` (the `429` reopen wait when no usable `Retry-After` is
  present), and a new `retry_after_cap` bounds an honored `Retry-After` (both validated
  non-zero at `stack()`/`build()`).
```

- [ ] **Step 4: Full CI gate**

Run: `just ci`
Expected: PASS — fmt, lint, test, doc, deny, typos all green. No new dependency, so
`deny` classifies nothing new.

Run: `just msrv`
Expected: PASS — builds on MSRV 1.90.

- [ ] **Step 5: Commit the docs**

```bash
git add docs/adr/0031-http-resilience-venue-pacing.md \
        docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md \
        CHANGELOG.md \
        docs/superpowers/specs/2026-07-08-net-http-retry-after-design.md \
        docs/superpowers/plans/2026-07-08-net-http-retry-after.md
git commit -m "docs(net): ADR-0031 Amendment #2 + changelog for Retry-After honoring"
```

- [ ] **Step 6: Open the issue and PR**

```bash
git push -u origin feat/net-http-retry-after
gh issue create \
  --title "feat(net): honor Retry-After (delay-seconds) on 429/5xx" \
  --label enhancement \
  --body "Tier-2 hardening item from #102: honor a delay-seconds Retry-After at two disjoint sites — the Retry 5xx backoff floor and the CircuitBreaker 429 reopen deadline. Renames throttle_cooldown->retry_after_fallback and adds retry_after_cap. 429 still never retried. Design: docs/superpowers/specs/2026-07-08-net-http-retry-after-design.md. Plan: docs/superpowers/plans/2026-07-08-net-http-retry-after.md."
gh pr create \
  --title "feat(net): honor Retry-After (delay-seconds) on 429/5xx" \
  --body "Closes #<ISSUE>. Refs #102. Honors a delay-seconds Retry-After at two disjoint sites (Retry 5xx backoff floor; CircuitBreaker 429 reopen), delay-seconds only, capped and panic-free. Breaking (pre-release): CircuitBreakerConfig::throttle_cooldown -> retry_after_fallback + new retry_after_cap. See docs/superpowers/plans/2026-07-08-net-http-retry-after.md."
```

(Fill `#<ISSUE>` from the created issue number.)

---

## Notes for the executor

- **Scope discipline.** This is a parser + two small layer edits + one config
  rename/add. If you find yourself touching `stack()`/`build()` *signatures*, the
  `Timer` trait, `RateLimit`, or adding a dependency, you have left the slice — stop and
  re-read the spec's Non-goals.
- **Why no `HTTP-date`.** The absolute form needs a wall-clock reference; `Timer` is
  monotonic-only (ADR-0029). `delay-seconds` covers the `429`/`503` case venues send.
- **Why the two caps differ.** Each layer bounds honoring with its own config
  (`RetryConfig::cap` for `5xx`, `CircuitBreakerConfig::retry_after_cap` for `429`) — no
  cross-layer config reach. This is the spec's reviewed "Open point" (per-layer caps).
- **ADRs are append-only.** Do **not** edit the §5 body or Amendment #1 of ADR-0031, or
  item #8 of ADR-0034 — the rename and honoring are recorded in the *new* amendments
  (0031 #2, 0034 #12), which is how the decision log stays truthful without rewriting
  history.

## Self-review (author)

- **Spec coverage:** parser (Task 1) ✓; Site 1 5xx floor + metric (Task 4) ✓; Site 2 429
  reopen + service extraction + metric (Task 5) ✓; rename + `retry_after_cap` +
  `validate_config` (Task 2) ✓; metric fn (Task 3) ✓; ADR-0031 #2 + ADR-0034 #12 +
  CHANGELOG (Task 6) ✓; delay-seconds-only, no new dep, 429-never-retried, no
  `stack()`/`build()` signature change — all preserved. Deferred items (HTTP-date, alt
  headers, RateLimit feed, 3xx) are explicitly out of scope.
- **Type consistency:** `parse_retry_after(&http::HeaderMap) -> Option<Duration>` is used
  identically in Tasks 4 and 5; `retry_after_honored(&'static str)` called with
  `"retry"`/`"breaker"`; `record(class, now, Option<Duration>)` matches every updated
  call site; `retry_after_fallback`/`retry_after_cap` field names are consistent across
  struct, arms, `validate_config`, and all construction sites.
- **Placeholders:** the only `#<ISSUE>` is the genuine, resolved-at-runtime PR body value
  (filled from the created issue). No TBD/TODO/"handle edge cases"/"similar to Task N".
