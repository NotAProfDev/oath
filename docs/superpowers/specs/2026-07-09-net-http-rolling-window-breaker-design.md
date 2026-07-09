# net-http rolling-window (error-rate) circuit breaker — design

## Context

The net-http resilience stack (ADR-0031) shipped its Tier-1 remediation in #104–#114,
and the first Tier-2 hardening item (`Retry-After` honoring) in #117. This spec covers
the next **Tier-2 hardening** item tracked in
[issue #102](https://github.com/NotAProfDev/oath/issues/102): replace the breaker's
**consecutive-count** trip trigger with a **rolling error-rate window**.

Today `CircuitBreaker`
([circuit_breaker.rs](../../../crates/adapter/net/http/api/src/circuit_breaker.rs))
trips `Closed → Open` after `failure_threshold` **consecutive** `Failure` outcomes
([circuit_breaker.rs:232-243](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L232-L243)).
The load-bearing word is *consecutive*: a single `Success` resets the streak to zero
([:253-257](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L253-L257)). So
a venue failing **50 %** of requests in an interleaved `F S F S …` pattern **never
trips** — the streak never reaches the threshold. For a trading venue this is the worst
regime: a half-broken gateway keeps swallowing orders into ambiguity while every
intervening success silences the alarm. ADR-0031 §5 already anticipated this
(*"consecutive-count for v1; rolling-window later"*,
[0031 §5](../../adr/0031-http-resilience-venue-pacing.md#L135)); the deep-review §2B
ranked it the top resilience-detection hole.

The existing anti-masking on `Class::Ignored` (a 4xx neither trips **nor resets**,
[:252](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L252)) is a *partial*
pre-emptive step. This spec generalizes it: by removing the reset-on-success cliff
entirely, interleaved **2xx** can no longer mask a building failure rate either.

### Research grounding (prior art)

- **resilience4j / Polly / Finagle / Envoy outlier detection** all reject
  consecutive-count for exactly this reason and trip on a **failure rate over a sliding
  window**, gated by a **minimum number of calls** (don't act on `1/1 = 100 %`).
- **`tower-resilience-circuitbreaker`**
  ([joshrotenberg/tower-resilience](https://github.com/joshrotenberg/tower-resilience))
  is direct prior art: `failure_rate_threshold(0.5)` + `sliding_window_size(N)` + the
  same three states, with named presets (`standard` = 50 %/100, `fast_fail` = 25 %/20,
  `tolerant` = 75 %/200). We deliberately **do not adopt** it — it is built on
  `tower::Service` (`poll_ready`, `&mut self`, associated `Future`), whereas OATH's
  stack is the RPITIT `oath_adapter_net_api::Service` (`&self`, no `poll_ready`, no
  `dyn`), which the deep-review §0 judged *superior for this use case*. Adopting it
  would mean either a full tower rewrite (rejected by three clean-slate architects) or a
  bridge shim more complex than the surgical change below — and its **generic** breaker
  cannot express OATH's venue semantics (two open-durations by trip cause, `Retry-After`
  honoring, the C1 local-`Throttled`-vs-venue-`429` `classify`). It is cited as prior
  art confirming the window/rate/min-calls shape and preset framing.

### Governing ADRs

- **ADR-0031 §5** — the `CircuitBreaker` backstop; states the v1 consecutive-count with
  *"rolling-window later"*. **Amendment #1** (C1) fixed `classify`; **Amendment #2**
  (#117) added `Retry-After` honoring + the `retry_after_fallback`/`retry_after_cap`
  fields. This feature is **ADR-0031 Amendment #3**.
- **ADR-0029** — `Timer` exposes only monotonic `now() -> Instant`. The count-based
  window is **clock-free** and needs no new `Timer` surface (unlike the time-based
  alternative — see D2).

## Goal

Replace the Closed-state trip trigger with a count-based rolling error-rate window: the
pure `Breaker` gains a small `RateWindow` in its `Closed` payload; `record` feeds each
`Failure`/`Success` into it and trips when the failure **rate** crosses a threshold once
a **minimum sample count** is present. `Open`, `HalfOpen`, the probe guard, `Retry-After`
honoring, and the single-per-host sharing all stay **exactly as they are**. One PR: a new
`RateWindow` unit, a `CircuitBreakerConfig` field swap, boot validation, an `open`-reason
telemetry label, tests, and an ADR amendment. **No new dependency; RPITIT/`&self`/no-`dyn`
Service preserved; `stack()`/`build()` signatures unchanged.**

## Design decisions (locked)

| # | Decision | Rationale |
| --- | --- | --- |
| **D1** | **Pure rate window replaces consecutive-count** (not a hybrid "rate OR N-consecutive"). | Single clean semantic + one config surface + one trip reason to reason about and table-test. Matches resilience4j/Polly/tower-resilience, which have no consecutive path. |
| **D2** | **Count-based window (last `N` outcomes)**, not time-based. | The window is **clock-free** — no monotonic-seconds plumbing derived from `Instant` (the time-based design's one genuinely new seam). Fully deterministic to table-test. It also *trips* a low-volume-but-persistently-failing venue (the ring fills over time), where a time-based window might never reach `minimum_calls` within its span. |
| **D3** | **Accepted trade-off of D2: no idle-time self-heal.** A count-based ring forgets only via **new calls**, not the clock, so a stale failure patch lingers until fresh successes flush it (slow recovery registration at low volume). | Bounded in practice: within a `Closed` episode the window either trips (→ `HalfOpen` → **reset to a clean slate**, D8) or is flushed by new traffic. Acceptable for a venue backstop; documented in the ADR so it is a deliberate choice, not an oversight. |
| **D4** | **Sample set = `Failure` (failure + call) and `Success` (call only).** `Class::Ignored` (4xx/Auth/Unknown) **never enters** the window; a venue `429` (`Class::TripNow`) trips **immediately** and is **not** a window sample. | The rate reflects only transport + 5xx host-health. Preserves today's anti-masking (a 4xx can neither trip nor dilute) and matches resilience4j's `ignoreExceptions`. `429`/penalty-box is an explicit venue directive, orthogonal to silent degradation. |
| **D5** | **Trip test:** `len ≥ minimum_calls  &&  failures * 100 ≥ failure_rate_threshold * len`. Integer cross-multiply, **no float**; boundary is `≥` (rate *equal to* threshold trips). | Deterministic, cheap, and keeps float out of a resilience path. A rate trip reopens on the normal `cooldown` (unchanged) — the two open-durations by cause (rate → `cooldown`; `429` → `retry_after_*`) are preserved. |
| **D6** | **`CircuitBreakerConfig` swap:** drop `failure_threshold`; add `failure_rate_threshold: u8`, `window_size: NonZeroU32`, `minimum_calls: NonZeroU32`. Keep `cooldown`, `retry_after_fallback`, `retry_after_cap`, `half_open_probes`. | Breaking config change is cheap pre-release. `NonZeroU32` carries "≥ 1" for the two counts as a type invariant, as the two existing count fields already do. |
| **D7** | **Validate at boot in `stack::validate_config`**, *not* by making `CircuitBreakerLayer::new` fallible. Checks: `failure_rate_threshold ∈ 1..=100`; `minimum_calls ≤ window_size`. | Consistent with where the breaker's sibling `retry_after_*` zero-checks already live (#117) and #113's Duration validation — one boot gate, no constructor-signature churn across callers. |
| **D8** | **`RateWindow` resets to empty on `HalfOpen → Closed`.** | A recovered host earns a clean slate; a pre-trip failure patch must not instantly re-trip it. |
| **D9** | **Fixed inline ring; O(1) push; zero per-request allocation.** | Matches the codebase's no-alloc-hot-path ethos (M8, fixed in #111). The window lives once inside the single `Arc<Mutex<Breaker>>`; `admit`/`record` mutate in place. |
| **D10** | **Single global per-host breaker unchanged.** Per-key breakers stay a **separate** #102 item. | ADR-0031 §5: IBKR's penalty box is per-IP/venue-wide. Windowing is per-breaker regardless of how many breakers exist. |
| **D11** | **Telemetry:** add a `reason` label to the `to="open"` transition (`reason ∈ {rate, throttle, probe_failed, abandoned}`). **Defer** a continuous failure-rate gauge (YAGNI v1). | The deep-review §2C flagged breaker-transition observability; a rate-degradation trip and a `429` penalty-box trip demand different operator responses and today look identical. A gauge needs periodic sampling; the trip transition is what you alert on. |

## Architecture

The pure `Breaker` state machine is unchanged **except** the `Closed` payload; `Open`,
`HalfOpen`, `admit`, the probe guard, `on_abandoned_probe`, and `Retry-After` all carry
over verbatim.

```text
BreakerState::Closed { consecutive_failures: u32 }   ─────►   Closed { window: RateWindow }

record(class, now, retry_after):
  Closed:
    Failure  → window.push(Failure);  if window.should_trip(cfg) → Open{cooldown}   (reason=rate)
    Success  → window.push(Success)                                                  (no reset cliff)
    Ignored  → no-op                        (never a sample — anti-masking preserved)
    TripNow  → Open{ min(retry_after, cap) or fallback }                             (reason=throttle)
  HalfOpen / Open:  unchanged (probe budget, reopen, Retry-After)
  HalfOpen → Closed on probe success:  window = RateWindow::empty()                  (D8)
```

### New unit — `RateWindow`

Its own small, table-testable type (feed it a sequence of outcomes; assert `rate()` /
`should_trip()`), isolated from the state machine exactly as the pure `Breaker` is
isolated from the async shell.

```rust
/// A fixed-capacity ring of the last `N` breaker-relevant outcomes, tracking the
/// running failure rate. Clock-free: only `Failure`/`Success` enter (a 4xx/Auth is
/// never pushed; a 429 trips immediately elsewhere). Resets to empty on recovery.
struct RateWindow {
    slots: Box<[Outcome]>,   // capacity = window_size, allocated ONCE with the breaker
    head: usize,             // next write index (ring)
    len: u32,                // fills 0..=window_size
    failures: u32,           // running count over the live slots
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome { Failure, Success }

impl RateWindow {
    /// Push one outcome (O(1)); evict the oldest once full, keeping `failures` exact.
    fn push(&mut self, o: Outcome) {
        if self.len == self.slots.len() as u32 {
            if self.slots[self.head] == Outcome::Failure { self.failures -= 1; }
        } else {
            self.len += 1;
        }
        if o == Outcome::Failure { self.failures += 1; }
        self.slots[self.head] = o;
        self.head = (self.head + 1) % self.slots.len();
    }

    /// Trip iff enough samples AND rate ≥ threshold. Integer cross-multiply (D5).
    fn should_trip(&self, min_calls: u32, threshold_pct: u32) -> bool {
        self.len >= min_calls && self.failures * 100 >= threshold_pct * self.len
    }
}
```

> `slots` is a `Box<[Outcome]>` sized from `window_size` at construction — a **single**
> allocation living inside `Arc<Mutex<Breaker>>`, never per request. (An `enum Outcome`
> is one byte; `window_size = 50` ⇒ 50 bytes for the whole venue.) The exact indexing
> shown is illustrative — the implementation must satisfy the no-indexing lint (helper
> accessors / iterators), per the repo's `#[deny]` posture.

### Config change — `CircuitBreakerConfig`

```rust
pub struct CircuitBreakerConfig {
    pub failure_rate_threshold: u8,     // NEW: trip at ≥ this % (1..=100), validated at boot
    pub window_size: NonZeroU32,        // NEW: N = ring capacity (last-N outcomes)
    pub minimum_calls: NonZeroU32,      // NEW: floor before the rate can trip (≤ window_size)
    pub cooldown: Duration,             // rate-trip reopen (UNCHANGED)
    pub retry_after_fallback: Duration, // 429 reopen w/o header (UNCHANGED, Amendment #2)
    pub retry_after_cap: Duration,      // 429 Retry-After ceiling (UNCHANGED, Amendment #2)
    pub half_open_probes: NonZeroU32,   // (UNCHANGED)
}
```

`failure_threshold` is **removed**. `stack::validate_config`
([stack.rs:118](../../../crates/adapter/net/http/api/src/stack.rs#L118)) adds
`failure_rate_threshold ∈ 1..=100` and `minimum_calls ≤ window_size` checks alongside
the existing `retry_after_*` zero-checks (D7). `HttpConfig` nests
`CircuitBreakerConfig`, so `stack()`/`build()` signatures are unchanged; the field swap
is the only caller-visible break (pre-release, no external users).

**v1 `build()` defaults** (tunable per deployment, not hardcoded): `failure_rate_threshold
= 50`, `window_size = 50`, `minimum_calls = 10`. N = 50 ≈ 5 s of traffic at IBKR's
~10 req/s pacing — responsive for a backstop while giving a stable rate. Reference
profiles from tower-resilience: `standard` (50 %/100), `fast_fail` (25 %/20).

### Telemetry — extend the #112 facade (D11)

`record`/`on_abandoned_probe` surface *why* a transition to `Open` happened (derivable
from `(class, prior phase)`), so [meter.rs](../../../crates/adapter/net/http/api/src/meter.rs)'s
`breaker_transition` attaches a `reason` label on `to="open"` only:
`reason ∈ {rate, throttle, probe_failed, abandoned}` — low, bounded cardinality. The
existing `http_circuit_breaker_transitions_total{to}` counter is otherwise unchanged.
No continuous rate gauge in v1.

## Testing

TDD, table-first, in the existing inline `#[cfg(test)]` modules (`MockTimer` +
`ScriptLeaf`, as the current suite does). Each test must fail if its guard regresses.

**`RateWindow` unit:**
- rate math: integer cross-multiply; boundary `failures*100 == threshold*len` trips (`≥`).
- ring eviction/wrap: pushing `> N` outcomes keeps `failures` exact and `len == N`.
- `minimum_calls` gate: below it, even `100 %` failures do **not** trip.
- all-failure saturation trips **exactly at** `minimum_calls`.

**Pure `Breaker` (table tests):**
- **Motivating case:** an interleaved `F S F S …` 50 % venue **trips** once `len ≥ min_calls`
  (the exact scenario consecutive-count missed).
- No-regression: a hard outage (`F F F …`) trips at `minimum_calls`; a healthy stream
  never trips.
- `Ignored` burst: a flood of 4xx neither trips nor dilutes (not a sample).
- Reset-on-close: fail into a trip → probe succeeds → `Closed` with an **empty** window
  (a pre-trip patch does not instantly re-trip).
- 429 `TripNow` still trips immediately on `retry_after_fallback`/honored value
  (Amendment #2 regression guard), independent of window state.

**Service (`service_tests`):** a `ScriptLeaf` sequence that produces a sub-threshold
mix stays `Pass`; a mix crossing the threshold fast-rejects (`CircuitOpen`) without
touching the leaf; a tripping-transition emits `to="open", reason="rate"`.

## ADR impact

- **ADR-0031 Amendment #3** (append-only, decision text unedited): consecutive-count →
  count-based rolling error-rate window; the trip formula and sample-set semantics (D4/D5);
  the config swap (D6) and boot validation (D7); the count-based **no-idle-self-heal**
  trade-off (D3) as accepted; single global breaker unchanged (D10); the `open`-reason
  telemetry label (D11); cites `tower-resilience-circuitbreaker` as prior art;
  supersedes the `failure_threshold` field in §5's config sketch.

## Scope (in)

- New `RateWindow` unit in `net-http-api` + wiring into `BreakerState::Closed`.
- `record` Closed-arm rewrite (rate window in place of the streak); `Success` no longer
  hard-resets; `Ignored` still a no-op; `TripNow`/`HalfOpen`/`Open` arms unchanged.
- Window reset on `HalfOpen → Closed`.
- `CircuitBreakerConfig` field swap + `stack::validate_config` checks + updated
  `build()` defaults.
- `breaker_transition` `reason` label on `to="open"`.
- Tests (`RateWindow`, `Breaker`, service) + ADR-0031 Amendment #3 + `CHANGELOG.md`
  `[Unreleased]`.

## Non-goals (deferred — each its own follow-up)

| Deferred | Why |
| --- | --- |
| **Time-based sliding window** (last `T` seconds) | Needs a monotonic-seconds seam derived from `Instant`; count-based (D2) is simpler, clock-free, and detects low-volume sustained failure. The config could grow a `window` enum later without another ADR. |
| **Per-key circuit breakers** | Separate #102 item; independent of windowing (D10). |
| **Hybrid rate-OR-consecutive fast-trip** | Rejected (D1) for semantic/config simplicity; `minimum_calls` low keeps hard-outage trip latency acceptable. |
| **Continuous failure-rate gauge** | YAGNI v1 (D11); the trip transition + reason is the alertable signal. |
| **Slow-call-rate threshold** (resilience4j) — treating slow-but-2xx calls as failures | `Timeout` already covers the hard-timeout case; slow-success accounting is a separate concern. |

## Delivery

One issue (split from tracking #102), one branch `feat/rolling-window-breaker` in a
worktree under `.claude/worktrees/`, one squash-merged PR (`Closes #<issue>`, references
#102). `just ci` + `just msrv` green; the spec and plan are committed on the feature
branch (per the one-issue-one-PR rule — not to `main`).

## Open points for review

1. **v1 defaults** — `50 % / N=50 / min_calls=10`. Match tower-resilience `standard`
   (50 %/100) or lean `fast_fail` (25 %/20) instead? (Tunable, so low-stakes.)
2. **`reason` telemetry scope** — it's the one part touching the service shell (not just
   the pure core). Keep it in this PR (recommended — the observability is the point) or
   split it into a trivial follow-up to keep the core change minimal?
