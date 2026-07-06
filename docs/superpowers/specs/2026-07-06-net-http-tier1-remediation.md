# net-http Tier-1 remediation — spec & PR plan (2026-07-06)

Executable spec for fixing the **Tier-1** findings from the
[deep review](../plans/2026-07-06-net-http-deep-review.md) (which itself builds on
[Fable's audit](../plans/2026-07-05-net-http-audit-findings.md)). Rationale and per-finding
evidence live in those docs; this spec fixes the delivery: scope, PR sequence, acceptance
criteria, and what is deferred.

## Goal

Land every **confirmed defect** and every **additive best-practice fix that fits the existing
ADRs**, restoring correctness/safety/operability of the HTTP stack without re-opening any
approved architecture decision.

## Scope

**In (Tier 1):** all findings that are bugs or additive gaps compatible with ADR-0029/0030/
0031/0034 — see the PR table. Two small **append-only** ADR touches are in-process (C1 =
ADR-0031 §5 clarification; configurable TLS roots = ADR-0030 §7 amendment), mirroring how
ADR-0034 amended its predecessors.

**Out (deferred → tracking issues):**
- **Tier 2 (ADR-deferred hardening):** rolling-window / per-key circuit breaker, `Retry-After`
  honoring, total-elapsed retry budget, request priority/fairness, bounded parked-request
  backpressure, outbound-proxy support, response decompression, streaming stall `TimeoutBody`.
- **Tier 3 (greenfield re-cuts that contradict ADRs):** typed local-vs-venue error, typed
  directive envelope replacing `http::Extensions`, split rate/concurrency layers,
  buffering-as-a-layer. Each requires a new/superseding ADR.

Each deferred item becomes a GitHub `enhancement` issue referencing the deep-review section.

## Delivery model

- **One dedicated worktree** at `.claude/worktrees/net-http-tier1` (never the editor's
  checkout). One branch per PR off `main`, in dependency order; a PR that needs an unmerged
  predecessor stacks on it and rebases onto `main` after the predecessor merges.
- **One issue → one PR** (CLAUDE.md). Every PR: issue → branch (`fix/…` or `feat/…`) → **TDD
  (red→green)** → CHANGELOG `[Unreleased]` entry → `just ci` green → PR `Closes #N`.
- **All GitHub issues created up front** (8 Tier-1 units — PR 7 is split into 7a/7b, so 9 PRs
  total — plus the deferred Tier-2/3 tracking issues).
- Hard rules enforced: no `unwrap`/`expect`/indexing/`unsafe` in non-test code; `missing_docs`;
  edition 2024 / MSRV 1.90; conventional commits; clippy `all` deny-level.

## PR sequence & acceptance criteria

Order: **PR1 & PR2 first** (critical bug + IBKR-connectivity blocker, both independent off
`main`); PR3 & PR4 independent; **PR5 before PR6/PR8** (they depend on the new `RateScope` enum
and opaque `ResponseBody`); PR7a/7b/PR8 last.

### PR 1 — Breaker + telemetry correctness  ·  `net-http-api`  ·  non-breaking
- **C1:** `classify` maps **error-side** `ErrorKind::Throttled` → `Class::Ignored` (a local
  decision; the host was never reached); `Ok(status == 429)` still → `Class::TripNow`.
- **M1:** `Breaker::admit` reports whether the admitted call is a Half-Open **probe**;
  `ProbeGuard` is armed **only for probes** (a cancelled Closed-state call can no longer reopen
  a concurrent Half-Open episode).
- **M2:** `kind_label` gains an explicit `ErrorKind::CircuitOpen => "circuit_open"` arm (keep
  the `_` catch-all for future `#[non_exhaustive]` variants).
- **L1:** `Instant + Duration` uses `checked_add` with a saturating fallback in
  `circuit_breaker.rs` and `rate_limit.rs`.
- **ADR:** append an ADR-0031 §5 clarification (Throttled-error vs 429-status).
- **Acceptance:** new test — a local `HttpError::Throttled` through the full `stack()` does
  **not** open the breaker; the existing 429-status trip test still passes; probe-guard test
  proving a cancelled Closed-era call does not reopen a Half-Open episode; `circuit_open` label
  test. `just ci` green.

### PR 2 — TLS + connection security  ·  `net-http-hyper`  ·  non-breaking  ·  **IBKR blocker**
- Configurable trust anchors in `ConnConfig`: an enum selecting bundled `webpki-roots`
  (default), the OS trust store, or explicit custom roots — so the leaf can trust IBKR's
  self-signed localhost gateway. (The custom-root path already works in the leaf's TLS test;
  this exposes it through `hyper_leaf`.)
- `https_only`: default rejects plaintext `http://`; plaintext allowed only via an explicit
  opt-in flag (fixes **N2**). Test servers/tests move to the flag or TLS.
- **HTTP/2 keepalive PING** knobs in `ConnConfig` (`http2_keep_alive_interval` / `_timeout` /
  `_while_idle`) with sane defaults for a long-lived venue connection.
- **ADR:** append an ADR-0030 §7 amendment (root-store configurability; keepalive).
- **Acceptance:** TLS round-trip through `hyper_leaf` (not a hand-built client) against a
  self-signed cert via the custom-root option; `https_only` rejects an `http://` URL; keepalive
  config threads through. `just ci` green.

### PR 3 — hyper error mapping  ·  `net-http-hyper`  ·  non-breaking
- **H1:** `map_legacy_err` inspects the wrapped error (`hyper::Error::{is_incomplete_message,
  is_canceled, is_body_write_aborted}` + io-error sources) and maps post-connect
  connection-class failures → `HttpError::Connection` (retryable, breaker-visible).
- **H2:** `map_hyper_err` (body phase): incomplete-message/truncation → `Connection`.
- **L4:** drop the per-call `self.client.clone()` in the leaf.
- Flip `aborted_connection_surfaces_an_http_error` and `truncated_response_body_*` expectations
  from `Other` to `Connection`.
- **Acceptance:** the reset/truncation integration tests assert `Connection`; a `Retryable`
  request over a reset connection is retried; `just ci` green.

### PR 4 — secret hygiene + dependency trim  ·  `net-http-api` + workspace  ·  non-breaking
- **M3:** hand-written redacting `Debug` (`finish_non_exhaustive`, omit secret fields) for
  `Auth`, `SetHeaders`, `HttpConfig`.
- **N3:** `set_sensitive(true)` on `Authorization`-class header values stamped by adapters
  (belt-and-braces).
- **M7:** workspace `tokio` with minimal default features; each crate enables what it needs
  (`rt`/`net`/`time`; dev-deps add `macros`/`rt-multi-thread`/`test-util`).
- **L5:** remove the unused `tracing-subscriber` dev-dep from `net-http-hyper`; investigate the
  `ring` co-compilation and add a `cargo-deny` ban if it is not required.
- **L6:** trim `futures-util`/`tracing` to explicit features (verify workspace-wide impact).
- **Acceptance:** a test asserting `{:?}` on `SetHeaders`/`HttpConfig` does not render a secret
  value; `cargo tree` shows the trimmed tokio feature set; `just ci` (incl. `deny`) green.

### PR 5 — API shape  ·  `net-http-api`  ·  **breaking (pre-release, no external users)**
- **M4:** `RateLimit` reads `BufferMode`; returns `permit: None` for a buffered response
  (releases the concurrency slot at `call`-return, not caller-drain).
- **M5:** `stack()`/`build()` return `impl HttpClient<Body: Send> + Clone + Send + Sync +
  'static`.
- **M6:** `enum RateScope<K> { None, Global, Local(K), Both(K) }` (illegal `Local`+`None` state
  becomes unrepresentable).
- **M9:** `ResponseBody` wraps a **private** inner enum behind a struct + accessors; variants no
  longer public.
- **Acceptance:** a response body crosses `tokio::spawn` (the `spawn_local` workaround in
  `stack.rs` is removed); buffered concurrency permit released at `call`-return test; the
  invalid `RateScope` state no longer compiles; adapters cannot match `ResponseBody` internals.
  `just ci` green.

### PR 6 — performance polish  ·  `net-http-api`  ·  non-breaking  ·  *depends on PR5*
- **M8:** `acquire` uses fixed slots (`[Option<&Bucket>; 2]`-style), no per-request `Vec`.
- **L2:** `Retry` clones the request only when another attempt may follow (move the owned
  request on the final/only send).
- **L3:** decorrelate jitter — perturb `SplitMix64` state on `clone`/`layer` so cloned services
  do not replay identical backoff sequences (keep the no-`rand` posture).
- **Acceptance:** a clone-count test proving no clone on the terminal/ineligible send; two
  cloned `Retry` services produce **different** backoff sequences; `just ci` green.

### PR 7a — observability (metrics + cardinality)  ·  `net-http-api`  ·  minor
- Numeric telemetry (counters/histograms) for breaker Open/Half-Open/Close transitions,
  `Throttled` rejections, retry attempts/backoff, and rate/concurrency permit-wait — routed to
  the ADR-0014 Telemetry plane, secret-safe, low-cardinality.
- `route` label templating seam so ID-bearing IBKR paths don't explode metric/label cardinality.
- **ADR:** a short ADR-0014 note on the metrics source.
- **Acceptance:** metrics emitted for a tripped breaker and a throttled request in tests;
  `route` normalization test; `just ci` green.

### PR 7b — operability (shutdown + config validation)  ·  `net-http-api` + `net-http-hyper`  ·  minor
- Graceful shutdown / pool-drain seam on the hyper leaf (await/cancel in-flight, close
  connections cleanly rather than `RST`).
- Validate `HttpConfig` / `CircuitBreakerConfig` `Duration`s at construction → `BuildError`
  (reject `timeout == 0`, `cooldown == 0`, etc.), symmetric with pacing validation.
- **Acceptance:** shutdown test (in-flight request completes or is cleanly cancelled); zero-
  `Duration` config yields a `BuildError`; `just ci` green.

### PR 8 — test debt + docs  ·  all crates  ·  non-breaking
- **M10:** proactive wait+refill loop with `max_wait > 0`; `RateLimit`-outside-`Timeout`
  ordering; Half-Open + `TripNow` re-trip (if not covered by PR1); `Scope::Both` acquire order;
  retry backoff schedule pinning.
- Integration test of the assembled `stack()` over the **real** hyper leaf — and pin the two
  reasoned-not-observed findings (h2-keepalive reaping, never-hit-429 burst over-admission)
  with an observed test before treating them as behavior contracts.
- SplitMix64 golden-vector test; token-bucket refill-**rate** assertion; loom test for the
  shared `Mutex<TokenState>`/`Arc<Mutex<Breaker>>` (or a documented decision to defer loom).
- **L12** fix tautological tests; **L13** doctests on `stack`/`HttpClient`/`RateScope`/layer
  factories; **L7/L8** stale rustdoc.
- `examples/` + a README documenting the mandatory per-request extension protocol
  (`RateScope`/`Retryable`/`BufferMode`) — the most likely real-world C1 trigger.
- **Acceptance:** the 5 behavior gaps have tests that fail under the corresponding bug; the
  integration test exercises the real leaf; `just doc` clean (per repo convention); `just ci`
  green.

## Deferred → tracking issues (created up front)

Tier 2: rolling-window breaker · per-key breakers · `Retry-After` honoring · total-elapsed
retry budget · request priority/fairness · bounded parked-request backpressure · outbound-proxy
support · response decompression · streaming stall `TimeoutBody`.
Tier 3: typed local-vs-venue error · typed directive envelope · split rate/concurrency layers ·
buffering-as-a-layer. Each labeled `enhancement`, referencing the deep-review section.

## Risks & notes

- **Breaking change (PR5)** is safe pre-release (README "do not use"); no external users.
- **Two ADR amendments** are append-only clarifications, not reversals — within process.
- **`ring` investigation (PR4)** may find the co-compilation is transitive and unavoidable; if
  so, document rather than force-remove.
- **Metrics library choice (PR7a)** must respect the ADR-0029 dependency posture (runtime-
  neutral `net-http-api`) — prefer a facade (`metrics`) over a runtime-coupled exporter, or
  keep it as `tracing` structured events if a new dep is unwelcome. To be decided in the PR7a
  plan.
