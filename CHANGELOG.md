# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

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

### Added

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
