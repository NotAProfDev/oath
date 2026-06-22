# Coding Standards Review & Quality-Gate Unification — Design

**Date:** 2026-06-21
**Status:** Approved (pending spec review)
**Branch context:** `feat/githooks-improvements`

## Problem

The repository's configuration files (`rustfmt.toml`, `rust-toolchain.toml`,
`deny.toml`, `Justfile`, `Cargo.toml`, `.clippy.toml`, `_typos.toml`) are
individually well-maintained, but three problems remain:

1. **No standards benchmark.** The configs were written from experience, never
   systematically compared against current upstream best practices and industry
   standards. There is no documented rationale trail for why each setting has its
   value, nor a record of where we intentionally deviate.
2. **Installed tools are not enforced.** The devcontainer installs `typos`,
   `taplo`, `cargo-machete`, `cargo-nextest`, `gitleaks`, `actionlint`, and
   `shellcheck`, but none are wired into the Justfile or CI. `_typos.toml` exists
   but nothing runs it.
3. **Logic is duplicated and drift-prone.** CI re-declares the same
   `cargo fmt`/`clippy`/`test`/`doc` commands that the Justfile already defines.
   Git hooks are mid-rewrite (`.githooks/` removed) and the `just setup` recipe
   points at a directory that no longer exists.

## Guiding principle: the Justfile is the single source of truth

Every quality gate is defined **once** as a `just` recipe. Three consumers call
those recipes; none duplicate the logic:

- **Git hooks** → thin shell wrappers that call `just <recipe>`.
- **CI** → installs tooling, then calls `just` recipes (primarily `just ci`).
- **Humans / Claude** → `just <recipe>` directly.

This guarantees local, hook, CI, and assistant runs are identical. CI's
`ci.yml` shrinks: per-step cargo commands move *into* the Justfile.

## Phases

The work proceeds in five phases. Phase 0 gates the rest.

### Phase 0 — Standards audit (gap analysis)

Before any file changes, benchmark each config against authoritative external
standards and produce a **gap-analysis matrix**, so every subsequent change is
traceable to a documented rationale.

**Method:** web-researched. Pull current (2026) upstream recommendations for each
tool rather than relying on potentially stale static knowledge. Cite sources.

**Per-file benchmarks:**

| File | Compared against |
|---|---|
| `rustfmt.toml` | Rust Style Guide; rustfmt Configurations reference |
| `.clippy.toml` | Clippy configuration docs; lint-group conventions |
| `Cargo.toml` (lints / profiles / metadata) | The Cargo Book; Rust API Guidelines; community profile norms |
| `deny.toml` | cargo-deny v2 recommended configuration |
| `rust-toolchain.toml` | rustup overrides docs; reproducibility norms |
| `_typos.toml` | typos reference |
| `Justfile` / CI / hooks | just manual; Conventional Commits spec; GitHub Actions hardening guidance |

**Output:** a matrix with columns
`file → setting → our value → best-practice value → verdict → action`, where
verdict ∈ {aligned, gap, intentional deviation}. The completed matrix is written
into this spec's companion or appended here during implementation.

**Honesty constraint:** the audit will surface findings that contradict already-made
decisions (notably the floating `stable` toolchain and stable-only rustfmt). These
are recorded as **documented intentional deviations** — the decisions stand, but
the spec shows the standard was considered and a deliberate choice was made.

### Phase 1 — Justfile recipe taxonomy

Recipes are organized in four layers.

**Atomic checks (one tool each):**

| Recipe | Command |
|---|---|
| `fmt` | `cargo fmt --all -- --check` |
| `fmt-toml` | `taplo fmt --check` |
| `typos` | `typos` |
| `check` | `cargo check --workspace --all-targets --all-features` |
| `lint` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `test` | `cargo nextest run --workspace --all-features` then `cargo test --doc --workspace --all-features` (nextest does not run doctests) |
| `test-no-run` | `cargo nextest run --workspace --all-features --no-run` (compile-only; fast pre-commit gate) |
| `deny` | `cargo deny --all-features check` |
| `doc` | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --all-features` |
| `machete` | `cargo machete` |
| `gitleaks` | `gitleaks detect --no-banner` (git mode in hook context) |
| `actionlint` | `actionlint` |
| `shellcheck` | `shellcheck` over `.githooks/*` and `.devcontainer/*.sh` |
| `msrv` | `cargo +1.85 check --workspace --all-targets --all-features` |

**Git-helper checks (Bash, no new dependencies):**

| Recipe | Behavior |
|---|---|
| `check-branch` | Reject commits made directly on `main` |
| `check-merge-conflicts` | Grep staged files for conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`) |
| `check-large-files` | Reject staged files larger than **5 MiB** |
| `commit-msg FILE` | Validate the first line against a Conventional Commits **regex** (no external tool) |

**Hook aggregates (what the hooks call):**

| Recipe | Composes |
|---|---|
| `pre-commit` | `check-branch check-merge-conflicts check-large-files fmt fmt-toml typos lint test-no-run` |
| `pre-push` | `ci` |

**Top-level:**

| Recipe | Composes |
|---|---|
| `ci` | `fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck` |
| `setup` | `git config core.hooksPath .githooks` (retained) |

Conventional-commit validation is a Bash regex inside the `commit-msg` recipe —
no cocogitto or external linter — consistent with the everything-via-just ethos.

### Phase 2 — Git hooks (`.githooks/`)

Three thin `set -euo pipefail` scripts, each delegating to the Justfile so all
logic lives in one place:

- `.githooks/pre-commit` → `exec just pre-commit`
- `.githooks/commit-msg` → `exec just commit-msg "$1"`
- `.githooks/pre-push` → `exec just pre-push`

These are linted by `just shellcheck`, which restores meaning to the `setup`
recipe and the README's hook-activation instructions. This phase is the spec the
in-flight hook rewrite follows.

### Phase 3 — CI (`ci.yml`)

One primary `ci` job, wired to just so local and CI runs are identical:

checkout → install non-cargo tools (taplo/typos/machete via cargo;
gitleaks/actionlint/shellcheck via their installers) → `dtolnay/rust-toolchain`
→ `Swatinem/rust-cache` → **`just ci`** → upload docs artifact.

**Exception — the `msrv` job stays separate.** It requires a different toolchain
(`+1.85`); folding two toolchains into one job is messier than a tiny parallel
job that runs `just msrv`. This is the single approved deviation from
"everything in one job."

### Phase 4 — Targeted config fixes

Driven by the Phase 0 audit; the known set:

- **`.clippy.toml`**: add `msrv = "1.85"` so Clippy stops suggesting post-MSRV APIs.
- **`Cargo.toml`**: add `keywords`, `categories`, `readme` to `[workspace.package]`
  (per the repo's own `rust.instructions.md`); lint groups unchanged.
- **`rustfmt.toml`**: stays on stable; import-grouping TODOs remain commented.
- **`rust-toolchain.toml`**: stays floating `stable`.
- **`deny.toml`** / **`_typos.toml`**: no content changes; `_typos.toml` is now
  exercised by `just typos`.

The audit may add small, well-justified items here; anything contentious is raised
before applying.

## Out of scope (YAGNI)

- Nightly rustfmt; import-granularity / group-imports changes.
- Exact toolchain version pin.
- New lint groups; `arithmetic_side_effects`; restriction-lint expansion.
- External commit-message linter (regex is used instead).
- Dependabot or devcontainer changes.

## Success criteria

- A committed gap-analysis matrix benchmarking all seven configs against cited
  2026 upstream standards, with deviations documented.
- `just ci` runs the full suite (Rust + all previously-unwired tools) and is the
  exact command CI invokes.
- `just pre-commit`, `just commit-msg`, `just pre-push` exist and back the three
  `.githooks/` scripts; `just setup` is meaningful again.
- `.clippy.toml` carries `msrv = "1.85"`; `Cargo.toml` carries
  `keywords`/`categories`/`readme`.
- No quality-gate logic is duplicated between CI and the Justfile.
