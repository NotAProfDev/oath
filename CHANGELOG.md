# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Breaking (pre-release) — net compose vocabulary.** Renamed the transport-neutral
  composition machinery in `oath-adapter-net-api` to match its `Service`-agnostic role
  (ADR-0029 §3): `ServiceBuilder` → `LayerBuilder`, the `Layer::Service` associated type
  → `Layer::Wrapped`, and the `ServiceBuilder::service()` finalizer → `LayerBuilder::wrap()`.
  The composition *unit* is shared across transports, but the assembled *product* is
  transport-specific (an HTTP `Service` today, a WS reconnect connector per ADR-0033),
  so the output type no longer misnames itself `Service`. The per-transport `Service`
  request/reply trait in `oath-adapter-net-http-api` is unchanged. Also dropped the
  unused `Copy` derive from `Identity`/`Stack` (nothing copies them; `Clone` retained).
  No behaviour change.
- Began the ADR-0029 network-adapter repartition: `oath-adapter-net-api` is now the
  transport-neutral, **std-only** kernel (composition machinery + `ErrorKind` +
  the new runtime-neutral `Timer` clock); the `Service` request/reply contract moved
  into the new per-transport crate `oath-adapter-net-http-api`.
- Restructured the workspace to the process-aligned, spine-inverted crate topology
  of ADR-0009: deleted `oath-engine` and `oath-ingest-core`; split
  `oath-messaging-core` into `oath-bus-api` + `oath-event-log-api`; renamed the
  `*-core` trait crates to `<subsystem>/api`; moved the risk/execution/portfolio
  Policies under `core/`; relocated `oath-net-core` to `oath-adapter-net-api`; and
  added `oath-core-api`, `oath-core-kernel`, and the `oath-core`, `oath-strategy-host`,
  `oath-cli`, and `oath-supervisor` process crates.
- `MockTimer` relocated from `oath-adapter-net-http-mock` into a new dev-only
  `oath-adapter-net-mock` crate beside the `Timer` contract in
  `oath-adapter-net-api`, so the HTTP and (forthcoming) WebSocket mock stacks
  share one fake clock without cross-depending (ADR-0034 §Amendments.4).
  `oath-adapter-net-http-mock` now provides only `MockClient`/`MockBody`.
- **net-http:** trimmed the build's dependency-feature footprint. Workspace `tokio`
  drops `features = ["full"]` for minimal defaults; each crate opts into only what it
  uses (prod `net-http-hyper`: `time`; dev/test: `macros`/`rt`/`net`/`io-util`/
  `test-util`), keeping `fs`/`process`/`signal`/`io-std` out of downstream production
  binaries (M7). `futures-util` and `tracing` are pinned to explicit features
  (`default-features = false`), dropping the `futures-macro`/`tracing-attributes`
  proc-macro trees and `parking_lot`; the unused `tracing-subscriber` dev-dependency is
  removed from `net-http-hyper` (L6, L5). No behaviour or public-API change.
- **Breaking (pre-release) — net-http API shape.** Four related surface changes:
  `RateScope<K>` is now an enum (`None`/`Global`/`Local(K)`/`Both(K)`) carrying the
  endpoint key inline, so an illegal "local scope with no key" is unrepresentable
  (M6); `ResponseBody` is an **opaque** struct — its buffer-vs-stream arms are
  private, inspected via `is_buffered()`/`is_streaming()` and consumed through the
  `Body` trait (M9); `stack()`/`build()` now return
  `impl HttpClient<Body: Send> + Clone + Send + Sync + 'static`, so response bodies
  cross `tokio::spawn` without the `LocalSet`/`spawn_local` workaround (M5); and
  `RateLimit` releases a concurrency permit at `call`-return for
  `BufferMode::Buffer` responses instead of letting it ride the in-memory body until
  the caller drains it (M4). No external users (pre-release).
- **net-http performance (hot path).** `RateLimit::acquire` collects its ≤2 buckets
  into fixed slots instead of two per-request `Vec`s — no allocation on the pacing
  path (M8). `Retry` clones the request only when another attempt may follow; the
  terminal or only send moves the owned request (L2). `SplitMix64::clone` now
  decorrelates — a cloned `Retry` service draws a divergent jitter stream rather
  than replaying the parent's, so the ADR's clone-per-task pattern no longer
  synchronizes backoff across tasks (L3). Behaviour-preserving; no API change.
- **net-http:** closed the Tier-1 resilience **test debt** (M10) and documentation
  gaps (issue #101). Added regression tests for the rate-limiter wait+refill park loop
  (`max_wait > 0`), exact refill rate, `RateScope::Both` acquire order, RateLimit-
  outside-Timeout permit-wait, no burst over-admission, the Half-Open + 429 re-trip on
  `retry_after_fallback`, the Retry backoff doubling ladder, and a SplitMix64 golden
  vector; integration tests exercising the assembled `stack()` over the **real** hyper
  leaf (reset→retry, 429→breaker-trip, send-timeout) plus a positive HTTP/2-keepalive
  survival test. Added doctests for `stack`/`build`/`HttpClient`/`RateScope`/the layer
  factories, a worked `examples/` + README for the mandatory per-request extension
  protocol, and fixed stale rustdoc (L7/L8) and tautological rate-config tests (L12).
  Test/docs only — no behaviour or public-API change. The loom concurrency model is
  deliberately deferred (documented).
- **Breaking (pre-release) — net-http.** `CircuitBreakerConfig::throttle_cooldown` is
  renamed `retry_after_fallback` (the `429` reopen wait when no usable `Retry-After` is
  present), and a new `retry_after_cap` bounds an honored `Retry-After` (both validated
  non-zero at `stack()`/`build()`).

### Added

- **net-http operability.** `HyperLeaf::shutdown()` drains in-flight requests
  (`await`s until an `Arc`-shared in-flight count reaches zero) so pooled
  connections can be dropped without `RST`ing an in-flight order submission; it
  does not reject new calls (stop sending first). `stack()`/`build()` now validate
  `HttpConfig` `Duration`s at construction — `timeout`, `circuit_breaker.cooldown`,
  and `circuit_breaker.retry_after_fallback` must be non-zero (a zero would silently
  defeat the layer it configures) — returning a new `BuildError::ZeroDuration`,
  symmetric with the existing pacing-parameter validation. `rate_limit_max_wait` and
  the retry backoff may still be zero.
- **net-http numeric telemetry (ADR-0014 Telemetry plane).** The resilience stack
  now emits counters/histograms through the runtime-neutral `metrics` facade
  (downstream installs the recorder/exporter; a no-op with none installed):
  circuit-breaker phase transitions (`http_circuit_breaker_transitions_total{to}`),
  local pacing rejections (`http_rate_limit_throttled_total{route}`), retry attempts
  and backoff (`http_retry_attempts_total{route}`, `http_retry_backoff_seconds`),
  and pacing permit-wait (`http_rate_limit_permit_wait_seconds{route}`). Cardinality
  is bounded by a new `RouteTemplate` request extension (a low-cardinality route the
  adapter stamps, e.g. `/iserver/account/{id}/order/{id}`) — also used for the
  `Tracing` span's `route`, so ID-bearing paths no longer explode label cardinality.
  (ADR-0014 amended.)
- `oath-adapter-net-http-api` HTTP contract — `HttpError` (one concrete
  transport/middleware error; HTTP statuses pass through as `Ok(Response)`),
  `HttpClient` (blanket-impl'd `Service` sub-trait), `ResponseBody` (buffer-xor-
  stream, forwarding `Body` metadata), and `BufferMode`. New `oath-adapter-net-
  http-mock` test harness (`MockClient`, `MockBody`, `MockTimer`).
- `oath-adapter-net-http-api` construction seams — `AuthSource` (per-attempt
  credential stamping) with `NoAuth`, the `Auth` layer (innermost, so `Retry`
  re-stamps per attempt) and `SetHeaders` (static defaults outside `Auth`,
  dynamic wins), and `Guarded` (response body carrying an optional `async-lock`
  concurrency permit, released at the earlier of stream-end or drop). ADR-0034
  records the construction-surface decisions and the ADR-0030/0031 amendments.
- `oath-adapter-net-http-api` boot-time pacing coverage — the `RateKey` trait
  (finite universe via `all()`), the `LimitPolicy`/`LimitDecl` classification
  vocabulary, the total `RateLimitConfig<K>` map, `BuildError`, and the
  standalone `validate_coverage` check: an unclassified endpoint or an
  out-of-range policy param is a boot failure, not a first-live-order 429
  (ADR-0034 §3). Closes Slice 0 of the net-http construction surface.
- `oath-adapter-net-http-api` `RateLimit` resilience layer (Slice 1) — the
  `RateLimit<S, K, T>` service + `RateLimitLayer<K, T>` factory (`net-api::Layer`):
  proactive per-endpoint pacing (token-bucket + concurrency policies) built from a
  validated `RateLimitConfig`, driven by `net-api::Timer` (mockable clock). Adds the
  `RateScope`/`Scope` per-request directive (absent → fails closed; `None` → opt-out;
  a runtime coverage gap fails closed as `Throttled`, never sent). `LimitPolicy::
  TokenBucket` gains `per: Duration` for sub-1/second venue limits, and the
  ≤1-concurrency-permit invariant is a boot check (`BuildError::MultipleConcurrency`).
  (ADR-0031 §3–4.)
- `oath-adapter-net-http-api` `Timeout` resilience layer (Slice 1 PR 2) — the
  `Timeout<S, T>` service + `TimeoutLayer<T>` factory (`net-api::Layer`): bounds the
  send (inner call → response) against a `net-api::Timer` deadline, returning
  `HttpError::Timeout` when it elapses first (inner future dropped); body-transparent.
  Adds the `RequestTimeout(Duration)` per-request override extension (absent → the
  layer default). Response-future-only (ADR-0031 §1's "bounds the send, not the permit
  wait"); a streaming-body timeout is deferred. No new dependency. (ADR-0031 §1,
  ADR-0034.)
- `oath-adapter-net-http-api` `Retry` resilience layer (Slice 1 PR 3) — the
  `Retry<S, T>` service + `RetryLayer<T>` factory (`net-api::Layer`): re-issues an
  explicitly-eligible request (a `Retryable` marker extension; absent → never retried,
  fail-safe) on a transient failure (`HttpError::{Timeout, Connection}`) or a `5xx`
  response, with capped-exponential full-jitter backoff up to `max_attempts`. Never
  retries a 429 / other 4xx / `Auth` / `Throttled`; returns the last outcome verbatim on
  exhaustion; body-transparent (drops a superseded response, releasing its `Guarded`
  permit). Adds the `Retryable` marker + `RetryConfig` schedule; jitter via an internal
  seeded `SplitMix64` — no new dependency. (ADR-0031 §2, ADR-0034.)
- `oath-adapter-net-http-api` `CircuitBreaker` resilience layer (Slice 1 PR 4) — the
  `CircuitBreaker<S, T>` service + `CircuitBreakerLayer<T>` factory (`net-api::Layer`):
  the reactive backstop to `RateLimit`. Trips Open after `failure_threshold` consecutive
  `Connection`/`Timeout`/`5xx` failures, or immediately on a `Throttled`/429 with the long
  `retry_after_fallback`; fast-rejects with a new non-retryable `HttpError::CircuitOpen`
  (mapped to a new `ErrorKind::CircuitOpen`) without touching the inner stack; admits
  bounded Half-Open probes after cooldown (reached-host closes, failure re-opens). Pure
  clock-injected `Breaker` state machine (Closed/Open/Half-Open) behind a thin
  `Arc<Mutex<Breaker>>` + `Timer` shell; single per-host breaker; `now()`-only (no sleep,
  no new dependency). 4-class outcome partition so `4xx`/`Auth`/unclassified errors neither
  trip nor mask an outage. (ADR-0031 §5, ADR-0034.)
- `oath-adapter-net-http-api` `Tracing` resilience layer (Slice 1 PR 5) — the outermost
  `Tracing<S, T>` service + `TracingLayer<T>` factory (`net-api::Layer`): one `info` span
  per logical request (method, route, status, `ErrorKind`, latency, attempts), attached to
  the inner future via `tracing::Instrument` so downstream events — including `Retry`'s new
  per-attempt events — nest under it. Latency via `net-api::Timer` deltas; secret-safe by
  construction (reads only method, `uri().path()` with the query dropped, status,
  `ErrorKind`, and the clock — never headers or bodies); body-transparent. `Retry` now emits
  `debug` per-attempt/backoff events and records the final attempt count onto the ambient
  span (a no-op without a `Tracing` span). Routed to the ADR-0014 Telemetry plane. Adds the
  `tracing` facade (runtime dep) + `tracing-subscriber` (dev-dep). (ADR-0031 §6, ADR-0014,
  ADR-0034.)
- `oath-adapter-net-http-api` `stack()` assembly + `HttpConfig` (Slice 2, assembly) —
  `stack<S, T, A, K>()` composes the canonical resilience order (ADR-0031 §1)
  `Tracing(CircuitBreaker(Retry(RateLimit(Timeout(SetHeaders(Auth(leaf)))))))` over any
  leaf, returning `Result<impl HttpClient + Clone + Send + Sync + 'static, BuildError>`.
  It builds the fallible `RateLimit` layer first (running `validate_coverage` +
  `validate_concurrency_singleton`), so a coverage/param/singleton failure is a boot
  error before the rest is assembled. `HttpConfig` is the non-generic aggregate
  (`timeout`, `retry`, `circuit_breaker`, `headers`, `rate_limit_max_wait`); the pacing
  map, `auth`, and `timer` are separate arguments. Full-stack ordering invariants
  (CircuitBreaker-outside-Retry, RateLimit-inside-Retry, send-Timeout, per-attempt
  Auth, Scope fail-closed) are regression-tested over an inline leaf + `MockTimer`. No
  new dependency; no existing-layer change. (ADR-0031 §1, ADR-0034.) The hyper leaf +
  `build()` land in the following slice.
- **net-http hyper backend (transport).** New `oath-adapter-net-http-hyper` crate:
  `TokioTimer` (the tokio `Timer`), the pooled TLS leaf (`hyper_leaf`/`ConnConfig`/
  `HyperLeaf`) over hyper-util + rustls (aws-lc-rs, webpki-roots), the
  `hyper → HttpError` mapping, and `build()` delegating to `stack()`. Response
  bodies stream; buffering follows in PR B.
- **net-http hyper backend (buffering).** The hyper leaf now honours the
  per-request `BufferMode` (ADR-0030 §4): `BufferMode::Buffer` collects the
  response body to `Bytes` inside the retry boundary (`ResponseBody::buffered`);
  absent or `Stream` keeps the live streaming body. Additive — no signature,
  associated-type, or layer change. (#92)
- net-http construction-surface design refinements (ADR-0034 append-only
  Amendments 2026-07-04, spec updated) — an absent `RateLimit<K>` directive now
  **fails closed** (not "defaults to `Global`"), closing the last silent
  under-pacing path; `Guarded` releases its permit on the earliest of terminal
  frame, **mid-stream error**, or drop; the `async-lock` choice is re-grounded on
  the stated multi-backend goal (a non-tokio stack stays genuinely `tokio`-free);
  and `MockTimer` will relocate from `net-http-mock` into a shared dev-only
  `oath-adapter-net-mock` crate so the WS resilience slice shares one fake clock.
  Code changes land with their slices.
- `oath-adapter-net-ws-api` WebSocket contract (ADR-0032/0033) — `Frame`/`CloseFrame`
  (RFC 6455 frame vocabulary), `WsError` (one concrete transport error with
  `HasErrorKind`), the split owned halves (`WsSink` one-shot RPITIT send half with
  terminal `close(self)`; `WsSource` blanket `Stream` recv half), the epoch-stamped
  lifecycle watch channel (`ConnState`, `LifecycleSnapshot`, `Lifecycle`/
  `LifecycleSender` over runtime-neutral `async-watch`), and the `WsConnector` leaf
  seam. New `oath-adapter-net-ws-mock` test harness (`MockWsConnector`, `MockSink`,
  `MockSource`).
- WebSocket transport design: ADR-0032 (contract — untyped duplex frame channel,
  asymmetric `Stream`/RPITIT split, epoch-stamped lifecycle, `WsConnector` leaf,
  per-transport `AuthSource`) and ADR-0033 (resilience — reconnect actor over a
  runtime-neutral `Spawn` seam, two-axis layer stack, `watch`-of-`LifecycleSnapshot`,
  dual-bound drop-oldest buffer, send-side rate limit, and a circuit breaker that
  retries transient loss forever but surfaces permanent failure as `Unrecoverable`).
  Validated against IBKR, Binance, and Coinbase WebSocket semantics.
- `oath-model` numeric primitives — the root contract's first real content: `Price`
  (signed fixed-point `i128`), `Quantity` (unsigned `u128` magnitude), `Side`
  (`Buy`/`Sell`), and `ArithmeticError`, with checked `const fn` add/sub that error
  rather than wrap (ADR-0023/0027). Dropped `rust_decimal`, `uuid`, and `time` from
  `oath-model`; added `proptest` and `serde_json` as dev-dependencies.
- Cargo workspace scaffold (initial 10 domain crates; later restructured — see Changed).
- Workspace-level lint configuration: `rustc`, `clippy` (all, pedantic, nursery, cargo,
  and selected restriction-group lints), with test-code exemptions via `.clippy.toml`.
- Build profiles: `release` (LTO + abort-on-panic), `profiling`, `bench`.
- `rust-toolchain.toml` pinning Rust `1.96.0` (an exact stable release) with `rustfmt`,
  `clippy`, and `rust-analyzer` components.
- `rustfmt.toml` with edition 2024 and Unix line endings.
- `deny.toml` (cargo-deny v2) for license allowlisting, advisory checks, and source restrictions.
- CI workflow (GitHub Actions): fmt, check, clippy, test, doc, and cargo-deny steps with
  caching via `Swatinem/rust-cache`, concurrency cancellation, and an MSRV check job.
- Dependabot configuration for Cargo crates, GitHub Actions, and dev container features.
- Dev container on the official `rust:1.96.0-trixie` image (Rust version pinned to match
  `rust-toolchain.toml`) with the `common-utils`, `rust`, `git`, Docker-outside-of-Docker,
  and GitHub CLI features, plus automatic git hook activation.
- Pre-commit hook: `cargo fmt --check` and `cargo clippy -D warnings`.
- Dual license: MIT OR Apache-2.0.
- **net-http (hyper leaf):** `ConnConfig` gains configurable TLS trust anchors
  (`TlsTrust::{WebpkiRoots, CustomRoots}`) so the leaf can reach a self-signed venue
  gateway (e.g. IBKR Client Portal); an `allow_http` flag defaulting to **HTTPS-only**
  (plaintext is now explicit opt-in); and HTTP/2 keepalive-PING knobs. `net-http-hyper`
  now depends on `rustls` directly (was dev-only).
- **net-http:** the resilience stack now honors a `delay-seconds` `Retry-After`
  response header at two disjoint sites — as the `5xx` retry backoff floor
  (`min(cap, max(retry_after, jittered))`, un-jittered) and as the `429`
  circuit-breaker reopen deadline (`min(retry_after, retry_after_cap)`, else the
  `retry_after_fallback` default). `429` is still never retried. An `HTTP-date`,
  float, overflowing, or absent value falls back to existing behavior. A new
  site-labelled `http_retry_after_honored_total` metric. (ADR-0031 Amendment #2)

### Fixed

- **net-http:** a local pacing rejection (`HttpError::Throttled`, request never sent) no
  longer trips the circuit breaker into the ~15-minute throttle-cooldown penalty box; only
  a venue `429` *response* trips it (C1). The Half-Open cancellation guard is armed only for
  genuine probes, so a cancelled non-probe call can no longer reopen a concurrent Half-Open
  episode (M1). `ErrorKind::CircuitOpen` now carries its own `circuit_open` telemetry label
  instead of `unknown` (M2). Cooldown and permit-wait deadline arithmetic saturates instead
  of panicking on degenerate `Duration` configs (L1).
- **net-http:** `Auth`, `SetHeaders`, and `HttpConfig` now hand-write redacting `Debug` impls
  (like every other layer) instead of deriving `Debug` over an `AuthSource`/`HeaderMap` that
  can hold credentials or static API keys, so a `{:?}`/`tracing` of them no longer leaks
  secrets (M3).
- **net-http (hyper leaf):** post-connect transport failures — a reset/closed pooled
  connection, an incomplete-message truncation, a cancelled/aborted transfer — now map
  to `HttpError::Connection` (retryable and circuit-breaker-visible) instead of the
  invisible `Other`/`Unknown` they collapsed to (H1); this also restores
  `BufferMode::Buffer`'s intended full-body retry coverage (H2). The leaf no longer
  clones the pooled client per call (L4).
