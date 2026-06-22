# Devcontainer Improvements — Design

**Date:** 2026-06-21
**Repo:** OATH (Rust workspace)
**Status:** Approved design, pending implementation plan

## Goal

Improve the devcontainer so it (A) closes functionality gaps, (B) builds and rebuilds
faster and more reliably, (C) is easier to maintain, and (D) better mirrors the CI
environment. The headline new capability is the GitHub CLI (`gh`) for creating issues
and PRs from inside the container.

## Current state

- Image: `mcr.microsoft.com/devcontainers/rust:2-1-trixie`.
- One feature: `ghcr.io/devcontainers/features/docker-outside-of-docker:1` (moby false).
- `postCreateCommand` does everything in one line: `apt-get install libclang-dev gitleaks just`,
  `cargo install --locked cargo-deny`, then `just setup` (wires `git config core.hooksPath .githooks`).
- Cargo cache volume mount is present but commented out.
- Extensions: rust-analyzer (clippy check), dependi, even-better-toml, github-actions, claude-code.
- Git hooks (`commit-msg`, `pre-commit`, `pre-push`) are shell scripts under `.githooks/`.
- `Justfile` provides `fmt`, `check`, `lint`, `test`, `deny`, `doc`, `msrv`, `ci`, `setup`.

## Decisions

- Installation backbone: **devcontainer Features**, with **`postCreateCommand` fallback**
  for tools that have no good feature.
- Features **float to latest** (no version pinning).
- **MSRV toolchain is out of scope** (`just msrv` stays a host-only concern).
- **No cargo cache volume / persistent mounts** (kept simple).
- Rust tooling installed with **`cargo install --locked`** (not `cargo-binstall`), for
  build-from-source provenance/safety. Accepts slower container creation as the trade-off.
- **No sccache** (its payoff is limited without a persistent cache).
- `gh` authentication via **host credential forwarding** (Option A), with manual
  `gh auth login` documented as fallback (Option B). No secrets in the repo.

## Architecture: three install tiers

Each tool is placed where it is most reliable and fastest.

### Tier 1 — Devcontainer Features (declarative, layer-cached, float to latest)

- `ghcr.io/devcontainers/features/github-cli` — `gh` CLI (official feature).
- Existing `docker-outside-of-docker` retained.
- Community features for `just` / `gitleaks` used **only if** well-maintained at
  implementation time; otherwise these fall to apt (Tier 2).

### Tier 2 — apt in `postCreateCommand` (reliable on Debian trixie)

- `libclang-dev` — build dependency (bindgen/libclang for iceoryx2).
- `shellcheck` — lints the shell git hooks.
- `gitleaks` and/or `just` — if not installed via a Tier 1 feature.

### Tier 3 — `cargo install --locked` in `postCreateCommand`

Rust tools are installed from source via `cargo install --locked` (consistent with the
existing `cargo-deny` install). This is slower than downloading prebuilt binaries, but
is preferred here for build-from-source provenance/safety. Tools:

- `cargo-deny`
- `cargo-nextest`
- `cargo-mutants`
- `cargo-machete`
- `typos-cli`
- `taplo-cli`
- `just-lsp`

`actionlint` is installed via its official binary install script (it is a Go tool, not
a crate).

`postCreateCommand` ends with `just setup` to wire the git hooks, as today.

## Extensions & settings

**Add:**

- `vadimcn.vscode-lldb` (CodeLLDB) — Rust debugging
- `usernamehw.errorlens` — inline diagnostics
- `github.vscode-pull-request-github` — PRs/issues in-editor (pairs with `gh`)
- `EditorConfig.EditorConfig`
- `ms-azuretools.vscode-docker`
- `eamodio.gitlens` (GitLens)
- `DavidAnson.vscode-markdownlint`
- `mikestead.dotenv` (repo has a `.env`)
- `tekumara.typos-vscode` — live spell-check using the same `typos` engine/config as CLI/CI

**Remove:**

- `streetsidesoftware.code-spell-checker` — replaced by `typos` (lower false-positive
  rate on Rust identifiers; single source of truth via `_typos.toml`).

**Keep:**

- `rust-lang.rust-analyzer` (with `rust-analyzer.check.command: clippy`), `fill-labs.dependi`,
  `tamasfe.even-better-toml`, `github.vscode-github-actions`, `anthropic.claude-code`.

## `gh` authentication

Rely on VS Code Dev Containers **host credential forwarding** — when the host machine is
already authenticated with `gh`/git, the container inherits credentials automatically.
No `containerEnv` tokens, no secrets committed. Document `gh auth login` as the manual
fallback (in the devcontainer comment and/or README).

## Spell-check configuration

Add a starter **`_typos.toml`** at the repo root with an allowlist seed (e.g. `OATH`,
crate names, common acronyms) so the editor extension, git hooks, and CI all read one
config.

## Validation

After a container build/rebuild:

1. Confirm each tool resolves on PATH: `gh`, `shellcheck`, `cargo nextest`,
   `typos`, `taplo`, `just-lsp`, `actionlint`, `cargo deny`, `cargo mutants`, `cargo machete`.
2. Confirm `just ci` still passes (CI parity).
3. Confirm `gh auth status` reports authenticated (when host is authed).

## Out of scope / follow-ups

- MSRV toolchain installation in-container.
- Cargo cache volume / persistent mounts and `sccache` (intentionally excluded).
- Wiring `shellcheck`/`typos` into the `pre-commit` hook (flagged as a follow-up, not
  part of this change).
- Version-pinning features.
- Additional CLIs not requested (ripgrep, fd, bat, etc.).
