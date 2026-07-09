# Rolling-Window (Error-Rate) Circuit Breaker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `CircuitBreaker`'s consecutive-count trip trigger with a count-based rolling error-rate window, so a venue failing a sustained fraction of interleaved traffic is detected (ADR-0031 Amendment #3).

**Architecture:** A new clock-free `RateWindow` (ring of the last-N `Failure`/`Success` outcomes with a running failure count) lives in `BreakerState::Closed`. `Breaker::record` feeds each host-health outcome into it and trips `Open` when `failures/len ≥ threshold` once `len ≥ minimum_calls`. `Open`/`HalfOpen`/probe-guard/`Retry-After`/single-per-host sharing are unchanged. A `reason` label is added to the `to="open"` transition metric.

**Tech Stack:** Rust (edition 2024), `oath-adapter-net-http-api` crate, `std::collections::VecDeque`, `metrics` facade, `just` task runner, `nextest`.

## Global Constraints

- Edition **2024**, MSRV **1.90** (validate with `just msrv`).
- **No `unsafe`** (`unsafe_code = "deny"`).
- **No `unwrap`/`expect`/indexing** in non-test code (warned) — return `Result`, model errors with `thiserror`. Test code is exempt.
- **Document public items** (`missing_docs` warned). `pub(crate)`/private items are exempt but doc them where it aids clarity.
- Clippy **`all` is deny-level** — no new warnings.
- **Conventional Commits** (`commit-msg` hook), e.g. `feat(net):`, `test(net):`, `docs(net):`.
- **Per-task verification** must run `just check`, `just test`, `just lint`, **and `just doc`** (rustdoc intra-doc links break silently otherwise). `just ci` + `just msrv` green before the PR.
- Work happens in the existing worktree `.claude/worktrees/rolling-window-breaker` on branch `feat/rolling-window-breaker`. One squash-merged PR, `Closes #118`, references #102.
- Tests are **inline `#[cfg(test)]` modules** (this repo uses no `tests/` dirs).

---

### Task 1: `RateWindow` unit

A standalone, clock-free rolling window. No dependency on the rest of the change; fully unit-tested on its own.

**Files:**
- Create: `crates/adapter/net/http/api/src/rate_window.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs` (add `mod rate_window;`)

**Interfaces:**
- Produces: `pub(crate) enum Outcome { Failure, Success }`; `pub(crate) struct RateWindow` with `fn new(window_size: NonZeroU32) -> Self`, `fn push(&mut self, o: Outcome)`, `fn reset(&mut self)`, `fn len(&self) -> u32`, `fn should_trip(&self, min_calls: u32, threshold_pct: u32) -> bool`.

- [ ] **Step 1: Declare the module.** In `crates/adapter/net/http/api/src/lib.rs`, add the private module declaration next to the other private modules (`mod clock;`, `mod retry_after;`):

```rust
mod rate_window;
```

- [ ] **Step 2: Write the failing tests.** Create `crates/adapter/net/http/api/src/rate_window.rs` with the tests first (the type does not exist yet):

```rust
//! A fixed-capacity rolling window of recent breaker outcomes tracking the failure
//! rate for the error-rate trip policy (ADR-0031 Amendment #3).
//!
//! Clock-free: only host-health outcomes enter — a transport failure / `5xx`
//! ([`Outcome::Failure`]) or a reached-host `2xx`/`3xx` ([`Outcome::Success`]). A
//! `4xx`/`Auth` (`Class::Ignored`) is never pushed, and a venue `429` trips the breaker
//! immediately without a window sample. Reset to empty when the breaker recovers.

use std::collections::VecDeque;
use std::num::NonZeroU32;

/// One breaker-relevant, host-health-bearing outcome that enters the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A transport failure (`Connection`/`Timeout`) or a `5xx` response.
    Failure,
    /// A reached-host success (`2xx`/`3xx`).
    Success,
}

/// The last-`N` outcomes as a ring, with a running failure count so the rate is O(1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RateWindow {
    cap: usize,
    samples: VecDeque<Outcome>,
    failures: u32,
}

#[cfg(test)]
mod tests {
    use super::{Outcome, RateWindow};
    use std::num::NonZeroU32;

    fn win(cap: u32) -> RateWindow {
        RateWindow::new(NonZeroU32::new(cap).unwrap())
    }

    fn push_n(w: &mut RateWindow, o: Outcome, n: u32) {
        for _ in 0..n {
            w.push(o);
        }
    }

    #[test]
    fn empty_window_never_trips() {
        assert!(!win(50).should_trip(10, 50), "no samples < min_calls");
    }

    #[test]
    fn below_min_calls_never_trips_even_at_full_failure() {
        let mut w = win(50);
        push_n(&mut w, Outcome::Failure, 9); // 100% failure, but only 9 < min_calls 10
        assert!(!w.should_trip(10, 50));
    }

    #[test]
    fn all_failures_trips_exactly_at_min_calls() {
        let mut w = win(50);
        push_n(&mut w, Outcome::Failure, 9);
        assert!(!w.should_trip(10, 50), "9 samples");
        w.push(Outcome::Failure); // 10th
        assert!(w.should_trip(10, 50), "reached min_calls at 100%");
    }

    #[test]
    fn interleaved_fifty_percent_trips_at_threshold_fifty() {
        let mut w = win(50);
        for _ in 0..10 {
            w.push(Outcome::Failure);
            w.push(Outcome::Success);
        } // 10 F + 10 S = 20 samples, rate 50%
        assert!(
            w.should_trip(10, 50),
            "50% failure rate meets the 50% threshold (>= trips)"
        );
    }

    #[test]
    fn just_below_threshold_does_not_trip() {
        let mut w = win(100);
        push_n(&mut w, Outcome::Failure, 49);
        push_n(&mut w, Outcome::Success, 51); // 49/100 = 49% < 50%
        assert!(!w.should_trip(10, 50));
    }

    #[test]
    fn eviction_keeps_the_failure_count_exact() {
        let mut w = win(10);
        push_n(&mut w, Outcome::Failure, 10); // window full, all failures
        assert!(w.should_trip(10, 50));
        push_n(&mut w, Outcome::Success, 10); // evicts all 10 failures
        assert_eq!(w.len(), 10, "capacity holds");
        assert!(!w.should_trip(10, 50), "window is now all successes");
    }

    #[test]
    fn reset_clears_to_empty() {
        let mut w = win(50);
        push_n(&mut w, Outcome::Failure, 20);
        w.reset();
        assert_eq!(w.len(), 0);
        assert!(!w.should_trip(1, 1));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail.**

Run: `just test -p oath-adapter-net-http-api rate_window`
Expected: FAIL — `RateWindow::new`, `push`, `len`, `should_trip`, `reset` not found.

- [ ] **Step 4: Implement `RateWindow`.** Add the `impl` block after the struct (before the `#[cfg(test)]` module):

```rust
impl RateWindow {
    /// An empty window of capacity `window_size`. The single backing allocation is
    /// sized once here; `push` never reallocates (it evicts before exceeding `cap`).
    pub(crate) fn new(window_size: NonZeroU32) -> Self {
        let cap = window_size.get() as usize;
        Self {
            cap,
            samples: VecDeque::with_capacity(cap),
            failures: 0,
        }
    }

    /// Record one outcome (O(1)); evict the oldest once full, keeping `failures` exact.
    pub(crate) fn push(&mut self, o: Outcome) {
        if self.samples.len() == self.cap {
            // Window full: drop the oldest. It was counted, so if it was a Failure the
            // running count is >= 1 here and the decrement cannot underflow.
            if self.samples.pop_front() == Some(Outcome::Failure) {
                self.failures -= 1;
            }
        }
        if o == Outcome::Failure {
            self.failures += 1;
        }
        self.samples.push_back(o);
    }

    /// Reset to empty — a recovered host earns a clean slate (ADR-0031 Amendment #3).
    pub(crate) fn reset(&mut self) {
        self.samples.clear();
        self.failures = 0;
    }

    /// The current live sample count.
    pub(crate) fn len(&self) -> u32 {
        self.samples.len() as u32
    }

    /// Trip iff at least `min_calls` samples **and** failure rate >= `threshold_pct`.
    /// Integer cross-multiply — no float in the resilience path; `>=` trips. Widen to
    /// `u64` so `failures * 100` cannot overflow for a large `window_size`.
    pub(crate) fn should_trip(&self, min_calls: u32, threshold_pct: u32) -> bool {
        let len = self.len();
        len >= min_calls
            && u64::from(self.failures) * 100 >= u64::from(threshold_pct) * u64::from(len)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass.**

Run: `just test -p oath-adapter-net-http-api rate_window`
Expected: PASS (7 tests).

- [ ] **Step 6: Verify lint + doc.**

Run: `just lint && just doc`
Expected: no warnings; docs build.

- [ ] **Step 7: Commit.**

```bash
git add crates/adapter/net/http/api/src/rate_window.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): add RateWindow rolling error-rate unit (ADR-0031 Am#3)"
```

---

### Task 2: Swap the breaker Closed-state to the error-rate window

Atomic core change: the config field swap, boot validation, and the `Breaker` logic all move together (they share a compile unit). This is a refactor — write the new pure-`Breaker` tests that encode the target behavior, swap config + logic + every construction site + tests, and land green.

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (config struct, `Breaker`, module rustdoc, both test modules)
- Modify: `crates/adapter/net/http/api/src/rate.rs:126-152` (add two `BuildError` variants)
- Modify: `crates/adapter/net/http/api/src/stack.rs` (`validate_config` + doctest + test helper)
- Modify: `crates/adapter/net/http/hyper/src/build.rs` (doctest at :41 + `http_cfg` test at :127)
- Modify: `crates/adapter/net/http/hyper/examples/client_with_directives.rs:65`

**Interfaces:**
- Consumes: `RateWindow`, `Outcome` from Task 1.
- Produces: `CircuitBreakerConfig { failure_rate_threshold: u8, window_size: NonZeroU32, minimum_calls: NonZeroU32, cooldown: Duration, retry_after_fallback: Duration, retry_after_cap: Duration, half_open_probes: NonZeroU32 }` (no `failure_threshold`); `BuildError::RateThresholdRange(u8)` and `BuildError::MinCallsExceedWindow(u32, u32)`; `Breaker::record` unchanged signature (`record(&mut self, class: Class, now: Instant, retry_after: Option<Duration>)`).

- [ ] **Step 1: Swap the config struct.** In `circuit_breaker.rs`, replace the `failure_threshold` field ([:40-56](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L40-L56)). New struct:

```rust
/// The circuit breaker's thresholds, as plain `Copy` data (ADR-0031 §5, Amendment #3).
///
/// `window_size`, `minimum_calls`, and `half_open_probes` are `NonZeroU32` ("≥ 1" is a
/// type invariant); `failure_rate_threshold` and the `minimum_calls ≤ window_size`
/// relationship are validated at boot by `stack::validate_config`.
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Failure-rate percentage (`1..=100`) that trips the circuit over the rolling
    /// window; a 50 % host trips at `50`. Validated at boot.
    pub failure_rate_threshold: u8,
    /// Rolling window size: the last-N outcomes the failure rate is computed over.
    pub window_size: NonZeroU32,
    /// Minimum window samples before the rate can trip (cold-start floor). Must be
    /// `≤ window_size` (validated at boot).
    pub minimum_calls: NonZeroU32,
    /// The cooldown before Half-Open probing after a **rate** trip.
    pub cooldown: Duration,
    /// The `429` reopen wait when the response carries no usable `Retry-After`
    /// (the penalty-box fallback; ≈ 10–15 min for IBKR) — Amendment #2.
    pub retry_after_fallback: Duration,
    /// Ceiling on an honored `429` `Retry-After`: `reopen = min(retry_after, cap)` —
    /// Amendment #2.
    pub retry_after_cap: Duration,
    /// Probes admitted per Half-Open episode; all must reach the host to close.
    pub half_open_probes: NonZeroU32,
}
```

- [ ] **Step 2: Import the window + swap the `Closed` payload.** Near the top of `circuit_breaker.rs`, add to the `use` block:

```rust
use crate::rate_window::{Outcome, RateWindow};
```

Change `BreakerState`'s derive ([:115](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L115)) to drop `Copy` (`RateWindow` is not `Copy`), and swap the `Closed` variant ([:116-127](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L116-L127)):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum BreakerState {
    /// Passing requests; `window` accumulates outcomes toward the rate trip.
    Closed { window: RateWindow },
    /// Rejecting fast until `reopen_at`; then the next admit begins Half-Open.
    Open { reopen_at: Instant },
    /// Probing: `probes_left` may still be admitted, `successes_needed` must reach
    /// the host before the circuit closes.
    HalfOpen {
        probes_left: u32,
        successes_needed: u32,
    },
}
```

- [ ] **Step 3: Update `Breaker::new` (drop `const`) and rewrite `record`.** `RateWindow::new` allocates, so `Breaker::new` can no longer be `const`. Replace `Breaker::new` ([:184-191](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L184-L191)) and `record` ([:227-294](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L227-L294)):

```rust
/// A fresh breaker starts Closed with an empty window.
pub(crate) fn new(cfg: CircuitBreakerConfig) -> Self {
    Self {
        state: BreakerState::Closed {
            window: RateWindow::new(cfg.window_size),
        },
        cfg,
    }
}
```

```rust
/// Record a classified outcome, transitioning as ADR-0031 §5 (Amendment #3) dictates.
///
/// In Closed, `Failure`/`Success` feed the rolling window and a `Failure` trips when
/// the rate crosses the threshold (with enough samples); `Ignored` is never a sample;
/// a `429` `TripNow` trips immediately (unchanged). `retry_after` is consulted only in
/// the `TripNow` arms, clamped to `retry_after_cap`.
pub(crate) fn record(&mut self, class: Class, now: Instant, retry_after: Option<Duration>) {
    // Hoist config reads (all `Copy`) so the `&mut self.state` match below borrows only
    // `state`, and compute the next state, applying it after the borrow ends.
    let min_calls = self.cfg.minimum_calls.get();
    let threshold = u32::from(self.cfg.failure_rate_threshold);
    let window_size = self.cfg.window_size;
    let rate_reopen = deadline(now, self.cfg.cooldown);
    let tripnow_reopen = deadline(
        now,
        retry_after.map_or(self.cfg.retry_after_fallback, |ra| {
            ra.min(self.cfg.retry_after_cap)
        }),
    );

    let next: Option<BreakerState> = match &mut self.state {
        BreakerState::Closed { window } => match class {
            Class::Failure => {
                window.push(Outcome::Failure);
                window
                    .should_trip(min_calls, threshold)
                    .then_some(BreakerState::Open {
                        reopen_at: rate_reopen,
                    })
            },
            Class::Success => {
                window.push(Outcome::Success); // dilutes the rate; no reset cliff
                None
            },
            Class::Ignored => None, // a 4xx/Auth is not a host-health sample
            Class::TripNow => Some(BreakerState::Open {
                reopen_at: tripnow_reopen,
            }),
        },
        BreakerState::HalfOpen {
            probes_left,
            successes_needed,
        } => match class {
            Class::Failure => Some(BreakerState::Open {
                reopen_at: rate_reopen,
            }),
            Class::TripNow => Some(BreakerState::Open {
                reopen_at: tripnow_reopen,
            }),
            // A reached-host probe (2xx/3xx or 4xx/Auth) resolves; the last one closes
            // to a fresh window.
            Class::Ignored | Class::Success => Some(if *successes_needed <= 1 {
                BreakerState::Closed {
                    window: RateWindow::new(window_size),
                }
            } else {
                BreakerState::HalfOpen {
                    probes_left: *probes_left,
                    successes_needed: *successes_needed - 1,
                }
            }),
        },
        // A stale outcome from a call admitted before a concurrent trip; drop it.
        BreakerState::Open { .. } => None,
    };
    if let Some(state) = next {
        self.state = state;
    }
}
```

> `admit`, `on_abandoned_probe`, and `phase` are **unchanged** — they already match `&mut self.state` / read discriminants only, which works for the now-non-`Copy` state.

- [ ] **Step 4: Update the module rustdoc.** In `circuit_breaker.rs`, the crate-module doc ([:1-22](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L1-L22)) references `failure_threshold`/"consecutive" — the intra-doc link would break `just doc`. Replace the first sentences of the module doc:

```rust
//! The `CircuitBreaker` resilience layer (ADR-0031 §5, Amendment #3): the reactive
//! 429/outage backstop to `RateLimit`'s proactive pacing.
//!
//! `RateLimit` tries never to hit a 429; `CircuitBreaker` stops cold if the host
//! fails anyway. It trips **Open** when the **failure rate** over the last
//! [`CircuitBreakerConfig::window_size`] outcomes reaches
//! [`CircuitBreakerConfig::failure_rate_threshold`] (once at least
//! [`CircuitBreakerConfig::minimum_calls`] samples are present), or **immediately** on a
//! venue **429 response** with the long [`CircuitBreakerConfig::retry_after_fallback`]
//! (IBKR's ~15-minute penalty box). A `Throttled` *error* and a `4xx`/`Auth` are local
//! or client-side and never enter the window.
```

Also fix the `Closed` variant doc comment ([:117](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L117)) — done in Step 2 above.

- [ ] **Step 5: Add the `BuildError` variants.** In `rate.rs`, inside `enum BuildError` ([:128-152](../../../crates/adapter/net/http/api/src/rate.rs#L128-L152)), before the closing brace, add:

```rust
    /// `circuit_breaker.failure_rate_threshold` is outside `1..=100` — a `0` would trip
    /// on the first sample and a value `> 100` could never trip.
    #[error("config field `circuit_breaker.failure_rate_threshold` must be in 1..=100, but is {0}")]
    RateThresholdRange(u8),
    /// `circuit_breaker.minimum_calls` (`{0}`) exceeds `window_size` (`{1}`) — the window
    /// can never hold enough samples to reach the floor, so the breaker could never trip
    /// on rate.
    #[error("config field `circuit_breaker.minimum_calls` ({0}) must be <= `window_size` ({1})")]
    MinCallsExceedWindow(u32, u32),
```

- [ ] **Step 6: Write the failing `validate_config` tests.** In `stack.rs`'s `#[cfg(test)] mod tests`, add (after the existing config tests):

```rust
#[test]
fn rejects_a_zero_failure_rate_threshold() {
    let mut cfg = http_cfg_for_validation();
    cfg.circuit_breaker.failure_rate_threshold = 0;
    assert_eq!(
        stack(Leaf, cfg, MockTimer::new(), NoAuth, total_rates()).err(),
        Some(BuildError::RateThresholdRange(0)),
    );
}

#[test]
fn rejects_an_over_100_failure_rate_threshold() {
    let mut cfg = http_cfg_for_validation();
    cfg.circuit_breaker.failure_rate_threshold = 101;
    assert_eq!(
        stack(Leaf, cfg, MockTimer::new(), NoAuth, total_rates()).err(),
        Some(BuildError::RateThresholdRange(101)),
    );
}

#[test]
fn rejects_min_calls_greater_than_window() {
    let mut cfg = http_cfg_for_validation();
    cfg.circuit_breaker.window_size = NonZeroU32::new(10).unwrap();
    cfg.circuit_breaker.minimum_calls = NonZeroU32::new(11).unwrap();
    assert_eq!(
        stack(Leaf, cfg, MockTimer::new(), NoAuth, total_rates()).err(),
        Some(BuildError::MinCallsExceedWindow(11, 10)),
    );
}
```

> Use the existing test's `Leaf`, `total_rates()`, and a small helper `http_cfg_for_validation()` that returns a valid `HttpConfig` — add it in this module mirroring the existing valid config already used by the passing `stack` test, with the new breaker fields (Step 8). If a valid `HttpConfig` builder already exists in this module, reuse it instead of adding one.

- [ ] **Step 7: Run the validation tests to verify they fail.**

Run: `just test -p oath-adapter-net-http-api validation`
Expected: FAIL — variants unused / config still has `failure_threshold` (won't compile until Step 8).

- [ ] **Step 8: Extend `validate_config` and swap every construction site.** In `stack.rs`, add the two checks to the `const fn validate_config` ([:192-208](../../../crates/adapter/net/http/api/src/stack.rs#L192-L208)) before `Ok(())`:

```rust
    if cfg.circuit_breaker.failure_rate_threshold == 0
        || cfg.circuit_breaker.failure_rate_threshold > 100
    {
        return Err(BuildError::RateThresholdRange(
            cfg.circuit_breaker.failure_rate_threshold,
        ));
    }
    if cfg.circuit_breaker.minimum_calls.get() > cfg.circuit_breaker.window_size.get() {
        return Err(BuildError::MinCallsExceedWindow(
            cfg.circuit_breaker.minimum_calls.get(),
            cfg.circuit_breaker.window_size.get(),
        ));
    }
```

Then replace the `failure_threshold: NonZeroU32::new(...).unwrap(),` line with the three new fields at **every** construction site. Use the recommended v1 profile `50 % / N=50 / min_calls=10` for the doc/example sites:

```rust
            failure_rate_threshold: 50,
            window_size: NonZeroU32::new(50).unwrap(),
            minimum_calls: NonZeroU32::new(10).unwrap(),
```

Sites (all currently `failure_threshold: NonZeroU32::new(3).unwrap()` unless noted):
- `stack.rs:129-135` (doctest in `stack()` doc)
- `stack.rs` `tests` module config helper (the `circuit_breaker: CircuitBreakerConfig { ... }` in `mod tests`)
- `build.rs:41` (single-line doctest — replace `failure_threshold: NonZeroU32::new(3).unwrap()` with the three fields inline)
- `build.rs:127-133` (`http_cfg()` test helper)
- `hyper/examples/client_with_directives.rs:65`
- `circuit_breaker.rs:345-352` (the `CircuitBreakerLayer::new` doctest)

- [ ] **Step 9: Rewrite the breaker test helpers + tests.** In `circuit_breaker.rs` `mod breaker_tests`, replace the `cfg(threshold, probes)` helper ([:611-619](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L611-L619)) with two helpers:

```rust
// General rate config for policy-specific tests.
fn rate_cfg(threshold_pct: u8, window: u32, min_calls: u32, probes: u32) -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        failure_rate_threshold: threshold_pct,
        window_size: NonZeroU32::new(window).unwrap(),
        minimum_calls: NonZeroU32::new(min_calls).unwrap(),
        cooldown: Duration::from_secs(30),
        retry_after_fallback: Duration::from_secs(900),
        retry_after_cap: Duration::from_secs(1800),
        half_open_probes: NonZeroU32::new(probes).unwrap(),
    }
}

// A config that trips on the FIRST failure (100% over a 1-sample window) — the analogue
// of the old `failure_threshold = 1`, for the Open/HalfOpen/probe tests whose behavior
// is independent of the trip policy.
fn first(probes: u32) -> CircuitBreakerConfig {
    rate_cfg(100, 1, 1, probes)
}
```

Apply this **mechanical substitution** to the existing tests (behavior unchanged — only the trip-policy config differs):

| Test (`mod breaker_tests`) | Old call | New call |
| --- | --- | --- |
| `throttle_trips_immediately_on_the_long_cooldown` | `cfg(3, 1)` | `first(1)` |
| `open_rejects_until_cooldown_then_admits_one_probe` | `cfg(1, 1)` | `first(1)` |
| `half_open_probe_success_closes` | `cfg(1, 1)` | `first(1)` |
| `half_open_probe_ignored_also_closes` | `cfg(1, 1)` | `first(1)` |
| `half_open_probe_failure_reopens` | `cfg(1, 1)` | `first(1)` |
| `half_open_probe_429_reopens_on_the_long_cooldown` | `cfg(1, 1)` | `first(1)` |
| `multi_probe_half_open_requires_all_to_close` | `cfg(1, 2)` | `first(2)` |
| `abandoned_probe_reopens_half_open` | `cfg(1, 1)` | `first(1)` |
| `abandoned_probe_is_a_noop_in_open` | `cfg(1, 1)` | `first(1)` |
| `record_while_open_never_untrips` | `cfg(1, 1)` | `first(1)` |
| `admit_distinguishes_a_probe_from_a_normal_pass` | `cfg(1, 1)` | `first(1)` |
| `a_429_retry_after_reopens_on_the_honored_value` | `cfg(1, 1)` | `first(1)` |
| `a_429_retry_after_is_clamped_to_the_cap` | `cfg(1, 1)` | `first(1)` |
| `a_429_without_retry_after_uses_the_fallback` | `cfg(1, 1)` | `first(1)` |

**Delete** these three now-obsolete consecutive-count tests: `closed_trips_after_threshold_consecutive_failures`, `a_success_resets_the_failure_streak`, `ignored_does_not_reset_the_streak`.

**Reframe** `abandoned_probe_is_a_noop_in_closed` (its intent — an abandoned call must not advance the Closed accumulator — survives) to:

```rust
#[test]
fn abandoned_probe_is_a_noop_in_closed() {
    let now = Instant::now();
    // Trip at 100% over a 3-sample window (min_calls 3).
    let mut b = Breaker::new(rate_cfg(100, 3, 3, 1));
    b.record(Class::Failure, now, None); // 1 sample
    b.record(Class::Failure, now, None); // 2 samples, < min 3 → not tripped
    b.on_abandoned_probe(now); // must NOT add a sample
    assert_eq!(
        b.admit(now),
        Admit::Pass,
        "2 real failures < min_calls 3 — abandon was a no-op"
    );
    b.record(Class::Failure, now, None); // 3rd real failure → 100% of 3 → trips
    assert_eq!(b.admit(now), Admit::Reject, "3rd real failure → tripped");
}
```

- [ ] **Step 10: Add the new rate-policy tests.** In `mod breaker_tests`, add:

```rust
#[test]
fn closed_trips_when_failure_rate_reaches_threshold() {
    let now = Instant::now();
    // 50% over a 20-window, min_calls 10.
    let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
    for _ in 0..5 {
        b.record(Class::Success, now, None);
        b.record(Class::Failure, now, None);
    } // 5F + 5S = 10 samples, 50%
    assert_eq!(
        b.admit(now),
        Admit::Reject,
        "50% over 10 samples meets the 50% threshold"
    );
}

#[test]
fn interleaved_successes_do_not_prevent_a_rate_trip() {
    // The motivating case: consecutive-count never tripped this; the rate window does.
    let now = Instant::now();
    let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
    for _ in 0..10 {
        b.record(Class::Failure, now, None);
        b.record(Class::Success, now, None); // a success no longer resets an alarm
    }
    assert_eq!(b.admit(now), Admit::Reject, "sustained 50% degradation trips");
}

#[test]
fn below_min_calls_never_trips() {
    let now = Instant::now();
    let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
    for _ in 0..9 {
        b.record(Class::Failure, now, None); // 100% but only 9 < min 10
    }
    assert_eq!(b.admit(now), Admit::Pass, "under the min-calls floor");
}

#[test]
fn ignored_is_not_a_window_sample() {
    let now = Instant::now();
    let mut b = Breaker::new(rate_cfg(50, 20, 10, 1));
    for _ in 0..30 {
        b.record(Class::Ignored, now, None); // a flood of 4xx: neither trips nor counts
    }
    assert_eq!(b.admit(now), Admit::Pass, "4xx never enters the window");
}

#[test]
fn window_resets_to_a_clean_slate_on_close() {
    let now = Instant::now();
    let mut b = Breaker::new(first(1)); // trips on the first failure
    b.record(Class::Failure, now, None); // → Open
    let after = now + Duration::from_secs(30);
    assert_eq!(b.admit(after), Admit::Probe);
    b.record(Class::Success, after, None); // probe closes → fresh empty window
    // The pre-trip failure did not carry over: one new failure at 100%/1 trips again,
    // proving the window is a clean slate (not still holding the old failure).
    assert_eq!(b.admit(after), Admit::Pass, "closed with an empty window");
}
```

- [ ] **Step 11: Update the `service_tests` helper.** In `mod service_tests`, replace the `cfg(threshold, cooldown, fallback, probes)` helper ([:1014-1027](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L1014-L1027)) so its first parameter drives the rate policy. Keep the same call arity by mapping `threshold` → a "trip after `threshold` consecutive failures at 100%" config (window = threshold, min_calls = threshold, rate = 100):

```rust
fn cfg(
    trip_after: u32, // consecutive failures needed at 100% (window = min_calls = trip_after)
    cooldown: Duration,
    fallback: Duration,
    probes: u32,
) -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        failure_rate_threshold: 100,
        window_size: NonZeroU32::new(trip_after).unwrap(),
        minimum_calls: NonZeroU32::new(trip_after).unwrap(),
        cooldown,
        retry_after_fallback: fallback,
        retry_after_cap: Duration::from_secs(1800),
        half_open_probes: NonZeroU32::new(probes).unwrap(),
    }
}
```

> This preserves every existing `service_tests` behavior: `cfg(3, …)` still trips after 3 straight failures (3/3 = 100% ≥ 100%), `cfg(2, …)` after 2, `cfg(1, …)` after 1 — because with no interleaved successes the rate stays 100%. No `service_tests` bodies change; only the helper does.

- [ ] **Step 12: Run the full crate tests.**

Run: `just test -p oath-adapter-net-http-api`
Expected: PASS. If any `service_tests` fails, it relied on a success *not* resetting — re-check against the helper mapping above.

- [ ] **Step 13: Full check + lint + doc.**

Run: `just check && just lint && just doc`
Expected: clean (the hyper crate's doctest/example + `build.rs` compile with the new fields).

- [ ] **Step 14: Commit.**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/rate.rs crates/adapter/net/http/api/src/stack.rs crates/adapter/net/http/hyper/src/build.rs crates/adapter/net/http/hyper/examples/client_with_directives.rs
git commit -m "feat(net)!: rolling error-rate breaker replaces consecutive-count (ADR-0031 Am#3)"
```

---

### Task 3: `open`-transition `reason` telemetry

Attach *why* the breaker opened, so a rate-degradation trip is distinguishable from a 429 penalty-box trip.

**Files:**
- Modify: `crates/adapter/net/http/api/src/circuit_breaker.rs` (`record`/`on_abandoned_probe` return a reason; three service call sites)
- Modify: `crates/adapter/net/http/api/src/meter.rs:60-63` + its test

**Interfaces:**
- Consumes: `Breaker::record`, `Breaker::on_abandoned_probe` from Task 2.
- Produces: `pub(crate) enum TripReason { Rate, Throttle, ProbeFailed, Abandoned }` with `const fn label(self) -> &'static str`; `record` and `on_abandoned_probe` return `Option<TripReason>`; `meter::breaker_transition(to: &'static str, reason: Option<&'static str>)`.

- [ ] **Step 1: Write the failing meter test.** In `meter.rs` `mod tests`, replace `breaker_transition_increments_a_phase_labelled_counter` so it asserts the `reason` label:

```rust
#[test]
fn breaker_transition_carries_to_and_optional_reason() {
    let recorder = DebuggingRecorder::new();
    let snap = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || {
        breaker_transition("open", Some("rate"));
        breaker_transition("half_open", None);
    });
    let snapshot = snap.snapshot().into_vec();
    let opened = snapshot.iter().any(|(k, _, _, v)| {
        k.key().name() == "http_circuit_breaker_transitions_total"
            && k.key().labels().any(|l| l.key() == "to" && l.value() == "open")
            && k.key().labels().any(|l| l.key() == "reason" && l.value() == "rate")
            && matches!(v, DebugValue::Counter(n) if *n == 1)
    });
    assert!(opened, "to=open carries reason=rate");
}
```

- [ ] **Step 2: Run it to verify it fails.**

Run: `just test -p oath-adapter-net-http-api breaker_transition`
Expected: FAIL — `breaker_transition` takes one argument.

- [ ] **Step 3: Extend the meter fn.** In `meter.rs`, replace `breaker_transition` ([:60-63](../../../crates/adapter/net/http/api/src/meter.rs#L60-L63)):

```rust
/// Record a circuit-breaker phase transition into `to`, with an optional `reason`
/// (present only for `to = "open"`: `"rate" | "throttle" | "probe_failed" | "abandoned"`).
pub(crate) fn breaker_transition(to: &'static str, reason: Option<&'static str>) {
    match reason {
        Some(reason) => {
            metrics::counter!(CIRCUIT_TRANSITIONS, "to" => to, "reason" => reason).increment(1);
        },
        None => metrics::counter!(CIRCUIT_TRANSITIONS, "to" => to).increment(1),
    }
}
```

- [ ] **Step 4: Return a reason from the breaker core.** In `circuit_breaker.rs`, add the enum near `Phase`:

```rust
/// Why the breaker transitioned to `Open` — a low-cardinality telemetry reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TripReason {
    /// The rolling failure rate crossed the threshold.
    Rate,
    /// A venue `429` response (`TripNow`) — the penalty box.
    Throttle,
    /// A Half-Open probe failed.
    ProbeFailed,
    /// A Half-Open probe was abandoned (cancelled / panicked).
    Abandoned,
}

impl TripReason {
    /// The stable, low-cardinality telemetry label.
    const fn label(self) -> &'static str {
        match self {
            Self::Rate => "rate",
            Self::Throttle => "throttle",
            Self::ProbeFailed => "probe_failed",
            Self::Abandoned => "abandoned",
        }
    }
}
```

Change `record` to return `Option<TripReason>` (Some only when it enters `Open`). Update the arms from Task 2: the Closed `Failure` rate-trip yields `Some(TripReason::Rate)`, Closed `TripNow` and HalfOpen `TripNow` yield `Some(TripReason::Throttle)`, HalfOpen `Failure` yields `Some(TripReason::ProbeFailed)`; all non-opening arms yield `None`. Concretely, make each arm return `(Option<BreakerState>, Option<TripReason>)` — or simpler, compute `next: Option<BreakerState>` as before and derive the reason alongside:

```rust
pub(crate) fn record(
    &mut self,
    class: Class,
    now: Instant,
    retry_after: Option<Duration>,
) -> Option<TripReason> {
    let min_calls = self.cfg.minimum_calls.get();
    let threshold = u32::from(self.cfg.failure_rate_threshold);
    let window_size = self.cfg.window_size;
    let rate_reopen = deadline(now, self.cfg.cooldown);
    let tripnow_reopen = deadline(
        now,
        retry_after.map_or(self.cfg.retry_after_fallback, |ra| {
            ra.min(self.cfg.retry_after_cap)
        }),
    );

    // (next state, trip reason). Reason is Some only when entering Open.
    let (next, reason): (Option<BreakerState>, Option<TripReason>) = match &mut self.state {
        BreakerState::Closed { window } => match class {
            Class::Failure => {
                window.push(Outcome::Failure);
                if window.should_trip(min_calls, threshold) {
                    (
                        Some(BreakerState::Open { reopen_at: rate_reopen }),
                        Some(TripReason::Rate),
                    )
                } else {
                    (None, None)
                }
            },
            Class::Success => {
                window.push(Outcome::Success);
                (None, None)
            },
            Class::Ignored => (None, None),
            Class::TripNow => (
                Some(BreakerState::Open { reopen_at: tripnow_reopen }),
                Some(TripReason::Throttle),
            ),
        },
        BreakerState::HalfOpen {
            probes_left,
            successes_needed,
        } => match class {
            Class::Failure => (
                Some(BreakerState::Open { reopen_at: rate_reopen }),
                Some(TripReason::ProbeFailed),
            ),
            Class::TripNow => (
                Some(BreakerState::Open { reopen_at: tripnow_reopen }),
                Some(TripReason::Throttle),
            ),
            Class::Ignored | Class::Success => (
                Some(if *successes_needed <= 1 {
                    BreakerState::Closed { window: RateWindow::new(window_size) }
                } else {
                    BreakerState::HalfOpen {
                        probes_left: *probes_left,
                        successes_needed: *successes_needed - 1,
                    }
                }),
                None,
            ),
        },
        BreakerState::Open { .. } => (None, None),
    };
    if let Some(state) = next {
        self.state = state;
    }
    reason
}
```

Change `on_abandoned_probe` ([:304-310](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L304-L310)) to return `Option<TripReason>`:

```rust
pub(crate) fn on_abandoned_probe(&mut self, now: Instant) -> Option<TripReason> {
    if matches!(self.state, BreakerState::HalfOpen { .. }) {
        self.state = BreakerState::Open {
            reopen_at: deadline(now, self.cfg.cooldown),
        };
        Some(TripReason::Abandoned)
    } else {
        None
    }
}
```

- [ ] **Step 5: Thread the reason through the three service call sites.** In `circuit_breaker.rs`, the three `breaker_transition(to)` calls become two-arg. The `record` site ([:530-542](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L530-L542)) captures the reason:

```rust
let (transition, reason) = {
    let now = self.timer.now();
    let mut breaker = self
        .breaker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = breaker.phase();
    let reason = breaker.record(class, now, retry_after);
    (transition_label(before, breaker.phase()), reason)
};
if let Some(to) = transition {
    crate::meter::breaker_transition(to, reason.map(super::TripReason::label));
}
```

The `admit` site ([:489-501](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L489-L501)) can only enter Half-Open (never Open), so it passes `None`:

```rust
if let Some(to) = transition {
    crate::meter::breaker_transition(to, None);
}
```

The `ProbeGuard::drop` site ([:455-466](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L455-L466)) captures the abandoned reason:

```rust
let (transition, reason) = {
    let mut breaker = self
        .breaker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = breaker.phase();
    let reason = breaker.on_abandoned_probe(now);
    (transition_label(before, breaker.phase()), reason)
};
if let Some(to) = transition {
    crate::meter::breaker_transition(to, reason.map(super::TripReason::label));
}
```

> `super::TripReason::label` is a `fn(TripReason) -> &'static str` used as `Option::map`'s closure. If the call sites are inside a nested `impl`, reference it as `TripReason::label` with a `use super::TripReason;` already in scope (`TripReason` is defined at module level).

- [ ] **Step 6: Update the existing `service_tests` reason assertion.** The `tripping_the_breaker_emits_an_open_transition_metric` test ([:1055-1079](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L1055-L1079)) trips via a `Connection` error → a **rate** trip (with `cfg(1, …)` = trip at first failure). Add a `reason=rate` assertion to it:

```rust
        let opened = snap.snapshot().into_vec().into_iter().any(|(k, _, _, v)| {
            k.key().name() == "http_circuit_breaker_transitions_total"
                && k.key().labels().any(|l| l.key() == "to" && l.value() == "open")
                && k.key().labels().any(|l| l.key() == "reason" && l.value() == "rate")
                && matches!(v, DebugValue::Counter(n) if n >= 1)
        });
        assert!(opened, "a rate trip emits to=open, reason=rate");
```

- [ ] **Step 7: Run tests + lint + doc.**

Run: `just test -p oath-adapter-net-http-api && just lint && just doc`
Expected: PASS, clean.

- [ ] **Step 8: Commit.**

```bash
git add crates/adapter/net/http/api/src/circuit_breaker.rs crates/adapter/net/http/api/src/meter.rs
git commit -m "feat(net): label breaker open-transitions with a trip reason"
```

---

### Task 4: ADR-0031 Amendment #3, CHANGELOG, final CI

**Files:**
- Modify: `docs/adr/0031-http-resilience-venue-pacing.md` (append Amendment #3)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Append ADR-0031 Amendment #3.** After Amendment #2 in `docs/adr/0031-http-resilience-venue-pacing.md`, add:

```markdown
3. **Consecutive-count trip → count-based rolling error-rate window.** §5 shipped the
   breaker with `failure_threshold` consecutive failures (*"consecutive-count for v1;
   rolling-window later"*). That is blind to mixed-traffic degradation — a single
   `Success` resets the streak, so a venue failing ~50 % of interleaved requests never
   trips (deep-review §2B). **Changed:** in Closed, `CircuitBreaker` now trips when the
   **failure rate** over the last `window_size` host-health outcomes reaches
   `failure_rate_threshold` (%), once at least `minimum_calls` samples are present —
   `len ≥ minimum_calls && failures*100 ≥ failure_rate_threshold*len` (integer, `≥`
   trips). Only `Failure` (transport/`5xx`) and `Success` (`2xx`/`3xx`) enter the window;
   a `4xx`/`Auth` (`Class::Ignored`) is never a sample, and a venue `429` (`TripNow`)
   still trips **immediately** on `retry_after_fallback`/honored value (Amendment #2,
   unchanged). The window is **count-based** (clock-free), chosen over time-based to
   avoid a monotonic-seconds `Timer` seam; the accepted trade-off is **no idle-time
   self-heal** — a stale failure patch lingers until flushed by new calls or a trip,
   which resets the window to a clean slate on recovery. `Open`, `HalfOpen`, the
   probe-guard, and single-per-host sharing are unchanged; per-key breakers remain
   deferred (#102). **Config:** `CircuitBreakerConfig` drops `failure_threshold` and
   gains `failure_rate_threshold: u8`, `window_size: NonZeroU32`, `minimum_calls:
   NonZeroU32`, validated at boot (`failure_rate_threshold ∈ 1..=100`, `minimum_calls ≤
   window_size`). The `to="open"` transition metric gains a `reason` label
   (`rate`/`throttle`/`probe_failed`/`abandoned`). Prior art: resilience4j / Polly /
   `tower-resilience-circuitbreaker` (rate + sliding window + minimum-calls); **not**
   adopted — it is built on `tower::Service`, whereas OATH keeps its RPITIT `&self`
   `Service`. Spec:
   `docs/superpowers/specs/2026-07-09-net-http-rolling-window-breaker-design.md`.
```

- [ ] **Step 2: Add the CHANGELOG entry.** Under `## [Unreleased]` → `### Changed` in `CHANGELOG.md`, add:

```markdown
- **Breaking (pre-release) — net-http circuit-breaker trip policy.** The `CircuitBreaker`
  now trips on a **rolling error-rate window** (last-N outcomes) rather than consecutive
  failures, so a venue failing a sustained fraction of interleaved traffic is detected
  (a 50 %-error host no longer resets an alarm on every success) — ADR-0031 Amendment #3.
  `CircuitBreakerConfig` drops `failure_threshold` and gains `failure_rate_threshold`
  (percent, `1..=100`), `window_size`, and `minimum_calls` (validated at boot). The
  `http_circuit_breaker_transitions_total{to="open"}` metric gains a `reason` label
  (`rate`/`throttle`/`probe_failed`/`abandoned`). Prior art: tower-resilience (not
  adopted — OATH keeps its RPITIT `Service`).
```

- [ ] **Step 3: Full local CI + MSRV.**

Run: `just ci && just msrv`
Expected: all green (fmt, lint, test, doc, deny, typos, MSRV).

- [ ] **Step 4: Commit.**

```bash
git add docs/adr/0031-http-resilience-venue-pacing.md CHANGELOG.md
git commit -m "docs(net): record ADR-0031 Amendment #3 + CHANGELOG (rolling-window breaker)"
```

- [ ] **Step 5: Push and open the PR.**

```bash
git push -u origin feat/rolling-window-breaker
gh pr create --fill --base main \
  --title "feat(net)!: rolling-window (error-rate) circuit breaker (ADR-0031 Am#3)" \
  --body "Closes #118. Replaces the CircuitBreaker's consecutive-count trip trigger with a count-based rolling error-rate window. References #102.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-Review

**Spec coverage** (each spec decision → task):
- D1 pure rate window (no hybrid) → Task 2 Step 3 (`record` rewrite).
- D2 count-based → Task 1 (`RateWindow`, `VecDeque`, clock-free).
- D3 no-idle-self-heal accepted → documented, Task 4 Step 1 (ADR).
- D4 sample set (Failure/Success in; Ignored out; 429 immediate) → Task 2 Steps 3, 10 (`ignored_is_not_a_window_sample`).
- D5 integer trip math, `≥` → Task 1 Step 4 (`should_trip`), Task 1 Step 2 (boundary test).
- D6 config swap → Task 2 Steps 1, 8.
- D7 boot validation → Task 2 Steps 5–8.
- D8 reset-on-close → Task 2 Step 3 (HalfOpen→Closed arm), Step 10 (`window_resets_to_a_clean_slate_on_close`).
- D9 zero-alloc ring → Task 1 (`VecDeque::with_capacity`, evict-before-exceed).
- D10 single global breaker unchanged → no layer/sharing change (untouched).
- D11 `reason` telemetry → Task 3.
- ADR-0031 Amendment #3 + CHANGELOG → Task 4.
- Motivating 50 %-venue test → Task 2 Step 10 (`interleaved_successes_do_not_prevent_a_rate_trip`).
- Regression guards (429 immediate, Retry-After) → preserved by Task 2 substitution table (`first(1)` tests).

**Placeholder scan:** none — all code shown; the only `<...>` is the PR number resolved by `#118`.

**Type consistency:** `RateWindow::{new,push,reset,len,should_trip}` and `Outcome::{Failure,Success}` are consistent across Tasks 1–2; `record(&mut self, Class, Instant, Option<Duration>) -> Option<TripReason>` (Task 3) and `on_abandoned_probe(&mut self, Instant) -> Option<TripReason>` match their three call sites; `breaker_transition(&'static str, Option<&'static str>)` matches all call sites and the meter test; `TripReason::label(self) -> &'static str` matches the `.map(...)` usage.
