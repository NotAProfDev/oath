# net-http `Retry-After` honoring (429 / 5xx) — design

## Context

The net-http resilience stack (ADR-0031) shipped its Tier-1 remediation in #104–#114.
This spec covers a **Tier-2 hardening** item tracked in
[issue #102](https://github.com/NotAProfDev/oath/issues/102): honor the venue-directed
`Retry-After` response header. It is **venue-directed backoff** — the one hardening
item a live trading client feels immediately, because a venue that tells us *exactly*
how long to wait lets us stop guessing.

Today two layers already peek `http::Response::status()` for their decisions but
ignore response **headers**:

- **`Retry`** ([retry.rs](../../../crates/adapter/net/http/api/src/retry.rs)) retries a
  `5xx` with **capped-exponential full-jitter** backoff, and **never** retries a `429`
  (ADR-0031 §2 — retrying compounds IBKR's penalty box).
- **`CircuitBreaker`**
  ([circuit_breaker.rs](../../../crates/adapter/net/http/api/src/circuit_breaker.rs))
  classifies a `429` *response* as `Class::TripNow` → opens **immediately** for the
  fixed `throttle_cooldown` (IBKR's ~10–15-minute penalty box).

`Retry-After` rides on both `503` (transient overload) and `429` (rate/penalty). This
spec wires it into **both** layers, at **disjoint** sites, so no response is paced
twice.

### Research grounding (2026-07-08 multi-source sweep)

- **RFC 9110 §10.2.3**: two forms — `delay-seconds` (a non-negative integer) or an
  `HTTP-date`. Both `429` (RFC 6585 §4) and `503` (§15.6.4) carry it as an explicit
  **MAY**; §10.2.3 has **zero RFC 2119 keywords for the client** — honoring, capping,
  or ignoring it is all spec-compliant. The grammar is **unbounded** (`1*DIGIT`), so a
  hostile/buggy `Retry-After: 259200` (3 days) is legal → **the honored value must be
  capped, and parsing must never panic** (the load-bearing anti-DoS rule; matches this
  repo's no-`unwrap`/no-`expect` lints).
- **Ecosystem gap**: *no* mainstream Rust retry crate honors `Retry-After` out of the
  box (reqwest-retry, retry-policies, tower::retry, backon, tryhard, again, backoff).
  retry-policies structurally *cannot* — its trait is `should_retry(start, n)`, blind
  to the response. OATH is filling a real gap, not reinventing a wheel.
- **No double-pacing** (resilience4j #2383, Polly): only *one* layer may pace a given
  response, and **the server value must not be re-jittered** (the server already
  jittered). OATH's `CircuitBreaker(Retry(...))` topology is already the correct one
  (the breaker sees a single post-retry outcome).

### Governing ADRs

- **ADR-0031 §2 / §5** — order-safe retry (never retry a `429`), and the `429` breaker
  backstop with the long `throttle_cooldown`. **Amendment #1** (C1 fix) established that
  only a `429` *response* trips (`TripNow`), never a local `Throttled` *error*. This
  feature is **ADR-0031 Amendment #2**.
- **ADR-0034 §4** — HTTP 4xx/5xx statuses are not error-ified; the resilience layers
  decide by **peeking** the response, "and the 429 `Retry-After` header — read-only; the
  response continues downstream unchanged." **Amendment #8** deferred "`Retry-After`
  parsing" as an additive follow-up — this spec lands its `delay-seconds` half.
- **ADR-0029** — `Timer` exposes only monotonic `now() -> Instant` + `sleep()`
  ([timer.rs](../../../crates/adapter/net/api/src/timer.rs)). We deliberately stay
  inside that seam (see D2).

## Goal

Honor a `delay-seconds` `Retry-After` at two disjoint sites — as the **`5xx` retry
backoff floor** and as the **`429` breaker reopen deadline** — bounded by a per-layer
cap, parsed without panic, falling back to existing behavior on any absent/unparsable
value. One small PR: a new parser module, a few lines in two existing layers, one
`record` parameter, one config rename + one new config field, tests, and an ADR
amendment. **No new dependency, no `stack()`/`build()` signature change, `429` still
never retried.**

## Design decisions (locked)

| # | Decision | Rationale |
| --- | --- | --- |
| **D1** | **Two disjoint sites.** `5xx` → `Retry` backoff only; `429` → `CircuitBreaker` reopen only. | No double-pacing (resilience4j #2383). A `429` is never paced by `Retry` (it isn't retried); a `5xx` is never paced by the breaker (it only *counts* it as `Failure`, tripping on threshold with the unrelated `cooldown`). |
| **D2** | **`delay-seconds` form only.** An `HTTP-date` (or any non-integer) → treated as absent. | The `HTTP-date` form needs a **wall-clock** reference, but `Timer` is monotonic-only; honoring it would require extending the ADR-0029 `Timer` contract with `system_now()`. `delay-seconds` is the form `429`/`503` limiters overwhelmingly send. Deferred, not dropped. |
| **D3** | **Server value overrides local backoff; never re-jittered.** | Industry consensus (Polly, resilience4j, AWS): the server already jittered — re-jittering undershoots the requested wait. |
| **D4** | **Cap the honored value (per layer, from its own config).** `5xx` → `RetryConfig::cap`; `429` → new `CircuitBreakerConfig::retry_after_cap`. | An unbounded `Retry-After` is a self-DoS vector. Each layer bounds honoring with its own ceiling — no cross-layer config coupling. |
| **D5** | **`429` reopen: `retry_after_cap` is independent of the no-header fallback**, so a directive *longer* than the fallback box is honored (up to the cap) rather than probed early. | Probing early into a legitimate long ban risks IBKR/Binance **escalation to a permanent block**. `retry_after_cap` may be set `≥ retry_after_fallback`. |
| **D6** | **Rename `throttle_cooldown` → `retry_after_fallback`; add `retry_after_cap`.** | The field's *only* use is the `429` reopen — i.e. "the wait when there is no usable `Retry-After`." Naming it as a matched `retry_after_*` pair (fallback / cap) is self-documenting. Breaking rename is cheap pre-release. |
| **D7** | **Parse is fallible and side-effect-free; unparsable ⇒ absent.** | `Retry-After` is an untrusted hint; a float `1.5`, an overflowing integer, an `HTTP-date`, or junk must fall back to existing behavior, never error or panic. |
| **D8** | **`429` is still never retried; honoring is additive.** No new per-request opt-in. | ADR-0031 §2 unchanged. Honoring is a response-driven refinement of *existing* layer behavior, not a new eligibility axis. |

## Architecture

```text
             ┌──────────────── CircuitBreaker ────────────────┐
             │  Ok(resp) 429 → reopen_at =                     │
 request ───►│    now + honored.map_or(retry_after_fallback,   │
             │                |ra| ra.min(retry_after_cap))    │   ◄── Site 2
             │                                                 │
             │   ┌──────────────── Retry ─────────────────┐    │
             │   │  Ok(resp) 5xx (retried) → backoff =     │    │
             │   │    min(cap, max(honored, jittered))     │    │   ◄── Site 1
             │   │  429 → passed through, NEVER retried    │    │
             │   └── RateLimit → Timeout → … → leaf ───────┘    │
             └─────────────────────────────────────────────────┘

   honored = crate::retry_after::parse_retry_after(resp.headers())   // Option<Duration>
```

### Shared parser — `crate::retry_after` (new module, zero dependency)

```rust
//! Parse the `Retry-After` response header (RFC 9110 §10.2.3), `delay-seconds` form.

use std::time::Duration;

/// The venue-directed wait from a `Retry-After` header, `delay-seconds` form only.
///
/// Returns `None` for an absent header, a non-ASCII value, or any non-integer form
/// (an `HTTP-date`, a float such as `1.5`, or junk) — the caller falls back to its
/// own default behavior. Never panics: `Duration::from_secs` is total over `u64`, and
/// an out-of-`u64` value simply fails to parse (`None`). The returned value is
/// **uncapped** — each caller clamps to its own ceiling (D4).
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
```

`pub(crate)` — both `retry.rs` and `circuit_breaker.rs` live in `oath-adapter-net-http-api`.
Add `mod retry_after;` to `lib.rs`.

### Site 1 — `Retry` (5xx backoff floor)

In [retry.rs](../../../crates/adapter/net/http/api/src/retry.rs)'s retry loop, once the
outcome is a to-be-retried `5xx`, read the header **before** the response is dropped,
then combine with the existing jittered ceiling:

```rust
let honored = match &outcome {
    Ok(resp) => crate::retry_after::parse_retry_after(resp.headers()),
    Err(_) => None,
};
drop(outcome); // release the prior response's Guarded permit (unchanged)
let jittered = self.rng.duration_in(backoff_ceiling(self.cfg.base, self.cfg.cap, attempt));
let delay = honored.map_or(jittered, |ra| ra.min(self.cfg.cap).max(jittered));
if honored.is_some() {
    crate::meter::retry_after_honored("retry");
}
```

`delay == min(cap, max(honored, jittered))`: the server value is a floor, the jittered
exponential is the *other* floor (never retry faster than our own schedule), the whole
thing bounded by the existing `RetryConfig::cap`, and the server value is **not**
re-jittered (D3). Net effect: honoring can only *lengthen* a `5xx` wait toward `cap` —
the safe direction for an overloaded server.

### Site 2 — `CircuitBreaker` (429 reopen deadline)

`Breaker::record` gains an `Option<Duration>` consulted **only** in the two `TripNow`
arms ([circuit_breaker.rs:234](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L234),
[:255](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L255)):

```rust
pub(crate) fn record(&mut self, class: Class, now: Instant, retry_after: Option<Duration>) {
    // … Class::TripNow arm (both Closed and HalfOpen):
    let cooldown = retry_after.map_or(self.cfg.retry_after_fallback, |ra| ra.min(self.cfg.retry_after_cap));
    self.state = BreakerState::Open { reopen_at: deadline(now, cooldown) };
}
```

The `CircuitBreaker` service (which holds the response) extracts the value on the
`429`-response path only and threads it in; every other call site passes `None`:

```rust
let retry_after = match &outcome {
    Ok(resp) if resp.status() == http::StatusCode::TOO_MANY_REQUESTS =>
        crate::retry_after::parse_retry_after(resp.headers()),
    _ => None,
};
// under the short record lock:
breaker.record(classify(&outcome), now, retry_after);
if retry_after.is_some() { crate::meter::retry_after_honored("breaker"); }
```

The pure `Breaker` core stays clock-injected; the honored duration is just another
injected input, so the state machine remains table-testable with zero async. `deadline`
([clock.rs](../../../crates/adapter/net/http/api/src/clock.rs)) already saturates (L1),
so a `now + cooldown` overflow is impossible.

### Config change — `CircuitBreakerConfig`

```rust
pub struct CircuitBreakerConfig {
    pub failure_threshold: NonZeroU32,
    pub cooldown: Duration,             // failure-threshold trips (UNCHANGED)
    pub retry_after_fallback: Duration, // RENAMED from `throttle_cooldown`: 429 reopen when no usable header
    pub retry_after_cap: Duration,      // NEW: ceiling on an honored 429 Retry-After
    pub half_open_probes: NonZeroU32,
}
```

`stack::validate_config`
([stack.rs:118](../../../crates/adapter/net/http/api/src/stack.rs#L118)) — rename the
existing `throttle_cooldown` zero-check to `retry_after_fallback`, and add a
`retry_after_cap.is_zero()` check (a zero cap would honor every `429` header as an
immediate probe — degenerate; reject at boot, symmetric with #113's Duration
validation). No ordering constraint between `retry_after_fallback` and `retry_after_cap`
is enforced, but the sensible relationship is `cap ≥ fallback` (documented on the
fields).

`HttpConfig` nests `CircuitBreakerConfig`, so `stack()`/`build()` signatures are
unchanged; the field rename is the only caller-visible break (pre-release, no external
users).

### Metrics — extend the #112 facade

Add to [meter.rs](../../../crates/adapter/net/http/api/src/meter.rs):

```rust
pub(crate) fn retry_after_honored(site: &'static str) {
    metrics::counter!(RETRY_AFTER_HONORED, "site" => site).increment(1);
}
```

Low-cardinality `site ∈ {"retry", "breaker"}` — lets operators see when a venue is
actively directing pacing vs. when local backoff/`fallback` dominates.

## Testing

All new tests go in the existing inline `#[cfg(test)]` modules (repo uses no `tests/`
dirs); each is **mutation-checked** — it must fail if its guard regresses.

**Parser** (`retry_after.rs`): `"120"` → `120s`; `"0"` → `0`; absent → `None`;
`"1.5"` → `None`; `"Wed, 21 Oct 2026 07:28:00 GMT"` → `None`; `"abc"` → `None`;
`"  120  "` → `120s` (trim); `"18446744073709551616"` (`u64::MAX + 1`) → `None` (no
panic); `"259200"` → `Some(3 days)` (parses; the *cap*, not the parser, bounds it).

**Site 1 — `Retry`**: a retried `5xx` carrying `Retry-After: N` waits `N` when
`N > jittered` and `N ≤ cap`; `Retry-After` **not** present → jittered unchanged;
`Retry-After < jittered` → jittered wins (floor); `Retry-After > cap` → capped at
`retry.cap`. (Use a fixed seed + `MockTimer`, mirroring the existing `drain` helper.)

**Site 2 — `CircuitBreaker`** (both `breaker_tests` pure-core and `service_tests`):
`429 + Retry-After: 2` → reopen at `now + 2s` (admit a probe at +2s, reject just
before); `429 + Retry-After: 259200` → capped at `retry_after_cap`; `429` with no
header → `retry_after_fallback` (the old `throttle_cooldown` behavior, unchanged);
Half-Open `429 + Retry-After` → same clamping. Existing breaker tests update to the new
field name **and** the new `record(class, now, None)` argument at every existing call
site (only the 429-response path passes `Some`).

## ADR impact

- **ADR-0031 Amendment #2** (append-only): the two disjoint honoring sites and their
  formulas; `delay-seconds`-only; the `throttle_cooldown` → `retry_after_fallback`
  rename + new `retry_after_cap` (updating §5's config sketch and noting the name used
  in Amendment #1 is superseded); reaffirm `429` is never retried and honoring is
  non-additive-pacing; `HTTP-date` + alternate/absolute headers deferred.
- **ADR-0034 Amendment #8**: one-line update — "`Retry-After` parsing" `delay-seconds`
  half has landed; `HTTP-date` still deferred (needs a `Timer` wall-clock seam).

## Scope (in)

- New `crate::retry_after` module + `parse_retry_after` (`delay-seconds`, zero-dep).
- `Retry` 5xx backoff-floor honoring (Site 1) + `retry_after_honored("retry")` metric.
- `CircuitBreaker` 429 reopen honoring (Site 2): `Breaker::record` gains
  `Option<Duration>`; service extracts on the 429 path; `retry_after_honored("breaker")`
  metric.
- `CircuitBreakerConfig`: `throttle_cooldown` → `retry_after_fallback`; add
  `retry_after_cap`; `validate_config` update.
- Tests (parser, Retry, breaker) + ADR-0031 Amendment #2 + ADR-0034 Amendment #8 note +
  `CHANGELOG.md` `[Unreleased]`.

## Non-goals (deferred — each its own follow-up)

| Deferred | Why |
| --- | --- |
| `HTTP-date` form of `Retry-After` | Needs a wall-clock `Timer::system_now()` seam (ADR-0029 change). `delay-seconds` covers the common `429`/`503` case. |
| Alternate/absolute headers (`X-RateLimit-Reset` epoch, `X-MBX-USED-WEIGHT`) | Absolute headers share the wall-clock problem; usage headers are proactive, not a delay. |
| Feeding `Retry-After` into `RateLimit` buckets (per-scope pre-emptive throttle) | A separate proactive-pacing concern; the disjoint-sites rule (D1) keeps this out. |
| `3xx` `Retry-After` (delayed redirects) | Not part of the trading pacing path. |
| Per-venue `honor / cap-and-honor / ignore` knob | The cap + safe-parse fallback already make honoring fail-safe; no knob to misconfigure. |

## Delivery

One issue (split from tracking #102), one branch `feat/net-http-retry-after` in a
worktree under `.claude/worktrees/`, one squash-merged PR (`Closes #<issue>`,
references #102). `just ci` + `just msrv` green; the spec and plan are committed on the
feature branch (per the one-issue-one-PR rule — not to `main`).

## Open point for review

**D4/D5 cap asymmetry:** the `5xx` honored value is bounded by the existing
`RetryConfig::cap`, while the `429` honored value is bounded by the new
`CircuitBreakerConfig::retry_after_cap`. This keeps each layer self-contained (no
cross-layer config reach), at the cost of two different ceilings. If a single unified
"max honored `Retry-After`" is preferred, it would live on `HttpConfig` and thread into
both layers — flag in review if so.
