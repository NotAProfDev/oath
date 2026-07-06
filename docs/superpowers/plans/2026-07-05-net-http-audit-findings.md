# net-http layer audit — consolidated findings (2026-07-05)

Multi-agent audit of `crates/adapter/net/http/{api,hyper,mock}` against industry best
practices, performance, and code flaws. Seven lens-specific reviewers (logic, async,
HTTP semantics, performance, module design, tests, security/hygiene) + a manual
end-to-end read. Findings below are deduplicated; **Confidence** reflects manual
verification against the code (`confirmed` = traced in source by a second reader;
`plausible` = reported by an agent, not yet independently re-verified).

> Status: 6 of 7 reviewer agents completed; the adversarial-verification and
> completeness-critic phases had not finished when this was written. Findings against
> `hyper/src/leaf.rs` were made pre-#92 (buffering merge); the two "New in #92" items
> below cover the new code.

## Critical

### C1. Locally-generated `Throttled` trips the circuit breaker into the 15-minute penalty box
- **Where:** [circuit_breaker.rs:79](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L79) (`classify`), interacting with [rate_limit.rs:350-353](../../../crates/adapter/net/http/api/src/rate_limit.rs#L350-L353)
- **Confidence:** confirmed (flagged independently by 5 of 6 reviewers)
- `HttpError::Throttled` is produced **only locally** by `RateLimit` — max_wait
  exhaustion, absent `RateScope` directive (fail-closed), missing key/bucket. It is
  never produced by the leaf; a real venue 429 arrives as `Ok(response)` with status
  429. But `classify` maps `ErrorKind::Throttled` to `Class::TripNow`, the same as a
  venue 429 → one local pacing rejection (request **never sent**) opens the venue-wide
  breaker for `throttle_cooldown` (≈15 min), fast-rejecting **all** traffic.
  The fail-closed safety design (ADR-0034 Amendment #1) makes this worse: one
  forgotten `RateScope` stamp = self-inflicted 15-minute outage.
- **Fix:** in `classify`, map the error-side `ErrorKind::Throttled` to
  `Class::Ignored` (local decision, host state unknown); keep status-429 → `TripNow`.
  Document as an ADR-0031 §5 clarification (the ADR conflated "Throttled/429").

## High

### H1. Post-connect transport failures map to `Other`/`Unknown` — invisible to Retry **and** CircuitBreaker
- **Where:** [error.rs:16-30](../../../crates/adapter/net/http/hyper/src/error.rs#L16-L30)
- **Confidence:** confirmed
- `map_legacy_err` only distinguishes `is_connect()`; a connection reset / dropped
  connection / truncated response on an **established** (e.g. pooled) connection maps
  to `HttpError::Other` → `ErrorKind::Unknown` → not transient for `Retry`, `Ignored`
  by the breaker. These are exactly the transients retry exists for (stale pooled
  connection reuse is the classic case), and repeated hard failures never trip the
  breaker. The leaf's own test acknowledges this (`aborted_connection_surfaces_an_http_error`
  asserts `Other`).
- **Fix:** inspect the wrapped `hyper::Error` (`Error::source()` /
  `hyper::Error::{is_incomplete_message, is_closed, is_canceled}` and io-error
  sources) and map connection-class failures to `HttpError::Connection`. Same for
  `map_hyper_err` (body-phase errors: incomplete message → `Connection`).

### H2 (New in #92). Buffered-mode collect errors are non-retryable
- **Where:** [leaf.rs:80-84](../../../crates/adapter/net/http/hyper/src/leaf.rs#L80-L84)
- **Confidence:** confirmed
- `BufferMode::Buffer` exists to give "full retry coverage" (body failures surface
  inside the retry boundary), but a mid-body reset during `collect()` maps via
  `map_hyper_err` to `Other` → never retried. Undercuts the feature's stated purpose.
  Fixing H1's mappers largely fixes this too.

## Medium

### M1. `ProbeGuard` is armed for every admitted call — a cancelled non-probe call can re-open someone else's Half-Open episode
- **Where:** [circuit_breaker.rs:406](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L406)
- **Confidence:** confirmed (race traced: call admitted in Closed → breaker trips →
  cooldown elapses → real probe admitted → the *old* call is cancelled →
  `on_abandoned_probe` re-opens, discarding the live probe's outcome)
- **Fix:** have `Breaker::admit` report whether the pass is a Half-Open probe; arm the
  guard only for probes. (A generation counter would also fix stale `record`s from
  pre-trip calls resolving probes; optional hardening.)

### M2. `kind_label` lacks a `CircuitOpen` arm — breaker fast-rejects logged as `error_kind="unknown"`
- **Where:** [trace.rs:30-40](../../../crates/adapter/net/http/api/src/trace.rs#L30-L40)
- **Confidence:** confirmed. Trivial fix: add `ErrorKind::CircuitOpen => "circuit_open"`.

### M3. Derived `Debug` on `Auth`, `SetHeaders`, `HttpConfig` can render credential material
- **Where:** [auth.rs:43](../../../crates/adapter/net/http/api/src/auth.rs#L43), [auth.rs:86](../../../crates/adapter/net/http/api/src/auth.rs#L86), [stack.rs:29](../../../crates/adapter/net/http/api/src/stack.rs#L29)
- **Confidence:** confirmed
- `Auth` derives `Debug` including the adapter's `AuthSource` (which may hold tokens);
  `SetHeaders`/`HttpConfig` derive `Debug` including `HeaderMap` (static API keys).
  `HeaderValue`'s `Debug` prints values unless `set_sensitive(true)`. Every other
  layer in the file already hand-writes redacting `Debug` impls — these three are the
  odd ones out.
- **Fix:** manual `Debug` impls (`finish_non_exhaustive`, omit `auth`/`headers`).

### M4. Buffered concurrency-scoped responses keep their permit until the caller drains the body (ADR divergence)
- **Where:** [rate_limit.rs:354-359](../../../crates/adapter/net/http/api/src/rate_limit.rs#L354-L359)
- **Confidence:** confirmed vs ADR-0034 §2 ("permit: None for … buffered responses")
- A `Full<Bytes>` body is not `is_end_stream()` until polled, so `Guarded::new` keeps
  the permit even though the transfer is already complete at `call`-return.
- **Fix:** `RateLimit` reads the request's `BufferMode` extension; on `Buffer` it
  returns `permit: None` (drops the permit at call-return).

### M5. `stack()`/`build()` return bound omits `Body: Send` — the composed client's responses can't cross `tokio::spawn`
- **Where:** [stack.rs:71](../../../crates/adapter/net/http/api/src/stack.rs#L71), [build.rs:28](../../../crates/adapter/net/http/hyper/src/build.rs#L28)
- **Confidence:** confirmed — stack.rs's own test works around it with `spawn_local`
  (line ~466 comment). Associated-type bounds are stable: return
  `impl HttpClient<Body: Send> + Clone + Send + Sync + 'static`.

### M6. `RateScope` makes invalid directives representable (`Scope::Local` with `key: None`)
- **Where:** [rate_limit.rs:33-39](../../../crates/adapter/net/http/api/src/rate_limit.rs#L33-L39)
- **Confidence:** confirmed (currently a runtime `Throttled`; could be a compile-time
  impossibility: `enum RateScope<K> { None, Global, Local(K), Both(K) }`). Pre-release,
  so the breaking change is cheap now.

### M7. `net-http-hyper` inherits `tokio` `features = ["full"]` into production code
- **Where:** [hyper/Cargo.toml](../../../crates/adapter/net/http/hyper/Cargo.toml), workspace [Cargo.toml:83](../../../Cargo.toml#L83)
- **Confidence:** confirmed — pulls fs/process/signal/io-std etc. into every
  downstream production binary. Fix: workspace `tokio` with minimal default features;
  crates add what they need (`rt`, `net`, `time`; dev-deps add `macros`,
  `rt-multi-thread`, `test-util`).

### M8. Per-request heap allocations in `RateLimit::acquire` for a ≤2-element bucket set
- **Where:** [rate_limit.rs:230-243](../../../crates/adapter/net/http/api/src/rate_limit.rs#L230-L243)
- **Confidence:** confirmed — two `Vec<&Bucket>` allocs per paced request; the set is
  statically ≤ 2 (global + local). Replace with fixed slots
  (`[Option<&Bucket>; 2]`-style), no allocation.

### M9. `ResponseBody` exposes public enum variants — leaks the buffer/stream machinery ADR-0030 §3 hides
- **Where:** [body.rs:24-38](../../../crates/adapter/net/http/api/src/body.rs#L24-L38)
- **Confidence:** confirmed (callers can match/construct variants around the
  `buffered`/`streaming` constructors). Fix: private inner enum behind a struct.

### M10. Test-coverage gaps in the resilience layers (from the tests lens)
- **Confidence:** confirmed by inspection of the cited test modules
  1. Token-bucket **wait/backpressure** loop: every rate test uses `max_wait = 0` —
     the sleep-then-reacquire path ([rate_limit.rs:288-306](../../../crates/adapter/net/http/api/src/rate_limit.rs#L288-L306)) is never exercised.
  2. `RateLimit`-outside-`Timeout` ordering (permit wait not bounded by send timeout)
     is untested — swapping the layers passes the suite.
  3. Half-Open re-trip on `TripNow` (429 during a probe → `throttle_cooldown`, not
     `cooldown`) is unpinned.
  4. `Scope::Both` + rate-before-concurrency/global-first acquire order never
     exercised through the service.
  5. Retry backoff schedule wiring unpinned (tests advance by `cap` each round; a
     doubling bug would pass).

## Low

- **L1.** `Instant + Duration` panics on degenerate configs: `now + max_wait`
  ([rate_limit.rs:232](../../../crates/adapter/net/http/api/src/rate_limit.rs#L232)), `now + cooldown` / `+ throttle_cooldown`
  ([circuit_breaker.rs:185-217](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L185-L217)). A `Duration::MAX` "no limit" sentinel
  panics at runtime. Use `checked_add` with a saturating fallback.
- **L2.** `Retry` clones the full request on every attempt even when retry is
  structurally impossible (`!eligible || max_attempts == 1`, and always on the final
  attempt) — [retry.rs:244](../../../crates/adapter/net/http/api/src/retry.rs#L244). Clone only when another attempt may follow.
- **L3.** `SplitMix64` clone/layer semantics give correlated jitter: every service
  from one `RetryLayer` starts at the same seed; clones snapshot state — concurrent
  clones draw **identical** backoff sequences ([retry.rs:68-75](../../../crates/adapter/net/http/api/src/retry.rs#L68-L75)). Perturb state
  on clone/layer (e.g. `fetch_add` a step on the parent).
- **L4.** `HyperLeaf::call` clones the pooled `Client` per request; the future may
  borrow `&self` per the `Service` contract — the clone is avoidable ([leaf.rs:63](../../../crates/adapter/net/http/hyper/src/leaf.rs#L63)).
- **L5.** Unused dev-dependency `tracing-subscriber` in `net-http-hyper`
  ([hyper/Cargo.toml](../../../crates/adapter/net/http/hyper/Cargo.toml)) — no reference in the crate.
- **L6.** `net-http-api` pulls default features of `futures-util`/`tracing`
  (proc-macro trees it doesn't use). Trim with `default-features = false` + explicit
  features (verify workspace-wide impact first).
- **L7.** Stale crate-level rustdoc in [api/src/lib.rs:26-27](../../../crates/adapter/net/http/api/src/lib.rs#L26-L27): "resilience layers,
  `stack`/`build` assembly, and backends land in later slices" — they've landed.
  Also [leaf.rs module doc](../../../crates/adapter/net/http/hyper/src/leaf.rs#L5) fixed by #92; re-check post-merge.
- **L8.** `validate_coverage`/`validate_concurrency_singleton` rustdoc says
  "`stack()`/`build()` call this" — actually `RateLimitLayer::new` does
  ([rate.rs:148-158](../../../crates/adapter/net/http/api/src/rate.rs#L148-L158)). Also reconsider whether both need root re-export when
  `RateLimitLayer::new` is the only door.
- **L9.** `MockClient` can't script per-call outcomes or return errors; five inline
  `ScriptLeaf` doubles exist across api test modules because of the dev-dep cycle.
  Consider giving `MockClient` a scripted-outcomes API for downstream adapter tests.
- **L10.** `MockBody` can't yield `Pending` or `Err` frames — mock semantics diverge
  from real streaming bodies ([mock/src/body.rs](../../../crates/adapter/net/http/mock/src/body.rs)).
- **L11.** Token-bucket wait loop has thundering-herd wakeups / no FIFO fairness under
  contention ([rate_limit.rs:288-306](../../../crates/adapter/net/http/api/src/rate_limit.rs#L288-L306)) — acceptable v1; document it.
- **L12.** Tautological tests in [rate.rs](../../../crates/adapter/net/http/api/src/rate.rs#L390) (`config_classifies_every_key_explicitly`,
  `token_bucket_carries_a_period_...` assert literals they just constructed).
- **L13.** No doctests on key public items (`stack`, `HttpClient`, `RateScope`,
  layer factories) across the three crates.

## Hardening notes (not defects; decide deliberately)

- **N1 (New in #92).** `BufferMode::Buffer` collects with **no size cap** — a
  misbehaving venue can allocate unbounded memory. Consider a max-buffer-bytes guard
  (checked against `size_hint().upper()` and enforced while collecting).
- **N2.** `https_or_http()` in the connector silently allows plaintext `http://`
  URIs in production. Consider an explicit `allow_http` flag in `ConnConfig`
  (IBKR's local gateway may need it; make it opt-in, not silent).
- **N3.** Consider `set_sensitive(true)` on `Authorization`-class header values
  stamped by adapters (belt-and-braces with M3; `http` skips them in Debug output).

## Suggested fix plan (one issue, one PR each, per CLAUDE.md)

1. **PR 1 — breaker + telemetry correctness (C1, M1, M2, L1):** `classify` fix +
   probe-only guard + `circuit_open` label + checked Instant arithmetic. Pure
   `net-http-api`, high value, no API break.
2. **PR 2 — hyper error mapping (H1, H2, L4):** connection-class mapping for
   legacy/hyper errors; drop the per-call client clone. Touches `net-http-hyper` only.
3. **PR 3 — secret hygiene + deps (M3, M7, L5, L6, N3):** manual Debug impls,
   tokio feature trim, dev-dep removal.
4. **PR 4 — API shape (M4, M5, M6, M9):** permit release for buffered mode,
   `Body: Send` return bound, `RateScope` enum, `ResponseBody` privacy. Breaking
   (pre-release, no external users).
5. **PR 5 — perf polish (M8, L2, L3):** acquire slots, conditional request clone,
   decorrelated jitter.
6. **PR 6 — test debt (M10, L9, L10, L12, L13, L7, L8):** the five behavior gaps +
   mock upgrades + doc fixes.
