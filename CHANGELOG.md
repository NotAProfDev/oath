# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace scaffold with 10 domain crates:
  `oath-model`, `oath-net-core`, `oath-messaging-core`, `oath-persistence-core`,
  `oath-ingest-core`, `oath-execution-core`, `oath-portfolio-core`, `oath-risk-core`,
  `oath-strategy-core`, `oath-engine`.
- Workspace-level lint configuration: `rustc`, `clippy` (all, pedantic, nursery, cargo,
  and selected restriction-group lints), with test-code exemptions via `.clippy.toml`.
- Build profiles: `release` (LTO + abort-on-panic), `profiling`, `bench`.
- `rust-toolchain.toml` pinning the stable channel with `rustfmt` and `clippy` components.
- `rustfmt.toml` with edition 2024 and Unix line endings.
- `deny.toml` (cargo-deny v2) for license allowlisting, advisory checks, and source restrictions.
- CI workflow (GitHub Actions): fmt, check, clippy, test, doc, and cargo-deny steps with
  caching via `Swatinem/rust-cache`, concurrency cancellation, and an MSRV check job.
- Dependabot configuration for Cargo crates, GitHub Actions, and dev container features.
- Dev container (`mcr.microsoft.com/devcontainers/rust:2-1-trixie`) with Docker-outside-of-Docker
  and automatic git hook activation.
- Pre-commit hook: `cargo fmt --check` and `cargo clippy -D warnings`.
- Dual license: MIT OR Apache-2.0.
