# Design: pipeline hardening — CodeQL + CodeRabbit

**Date:** 2026-06-22
**Status:** Approved (pending implementation plan)
**Topic:** Add two bug/quality-catching layers to the CI pipeline — GitHub
CodeQL (security SAST for Rust) and CodeRabbit (AI PR review) — scaled to a
pre-release, single-maintainer OSS Rust project.

## Goal

Catch bugs and quality issues **earlier**, before they land on `main`, without
adding redundant noise that a solo maintainer has to triage. This pass adds a
deterministic security scanner (CodeQL) and a reasoning-based PR reviewer
(CodeRabbit) on top of the already-strong existing gates. Supply-chain hardening
and posture/badges are explicitly **out of scope** for this pass.

## Context: what already exists

The pipeline is already well-hardened, so this is an additive next layer, not a
fix of missing basics. Already in place:

- `ci.yml` with top-level `permissions: {}`, per-job least privilege,
  `persist-credentials: false`, SHA-pinned + checksum-verified binary installs,
  concurrency cancel, timeouts, and a single `just ci` entry point.
- Secret scanning (gitleaks), `actionlint`, `shellcheck`.
- Full `cargo-deny` suite (advisories, licenses, bans, sources), `cargo-machete`,
  `typos`, `taplo`, `rustfmt`, clippy at **deny** level.
- MSRV (1.90) verification job.
- Dependabot across `cargo`, `github-actions`, `devcontainers` with cooldown and
  major/minor split.
- `CODEOWNERS`, `SECURITY.md` (private vulnerability reporting), pre-push hook
  enforcing the same `just ci`.

## Scope decisions (locked with the user)

- **In scope, build now:** CodeQL (Rust SAST) **and** CodeRabbit (AI PR review).
- **Out of scope / skipped:** test coverage (cargo-llvm-cov + Codecov) — revisit
  once backend crates carry real logic.
- **Deferred, documented only:** GitHub **Code Quality** (the maintainability
  product, GA **2026-07-20**) — evaluate after GA, adopt only if it catches
  things CodeRabbit + clippy miss.
- **SonarQube/SonarCloud:** rejected. Weak Rust support (largely commercial
  tier); clippy at deny-level already covers most of what it would flag for Rust.
- **CodeQL query suite:** **default** (high precision, low false-positive), not
  `security-extended`. Can expand later.
- **Branch protection:** recommended manual follow-up (make CodeQL a required
  status check on `main`) — a repo-settings change, not a file in this repo.

## Why these two, and how they divide the work

CodeQL and CodeRabbit are complementary, not redundant:

| Concern | Tool |
|---|---|
| Security-relevant bug patterns (deterministic) | **CodeQL** |
| General maintainability / code smells | **clippy** (today); GitHub Code Quality (deferred) |
| Broad PR review — logic, naming, edge cases, docs (reasoning) | **CodeRabbit** |

CodeQL is a deterministic ruler: repeatable, never hallucinates, only finds what
its rules encode. CodeRabbit is a reasoning reviewer: catches the
"a human would notice this" class, but is not perfectly repeatable. Stacking
*both plus* a future GitHub Code Quality *plus* clippy risks review fatigue for a
solo maintainer, which is why Code Quality is deferred rather than added now.

## Component 1 — CodeQL workflow

A new `.github/workflows/codeql.yml`, written to match existing repo conventions.

- **Engine:** `github/codeql-action` (`init` + `analyze`) with
  `languages: rust` and **`build-mode: none`**. Rust no-build scanning is GA
  (2025-10-14), so no Rust toolchain setup or compile step is required — keeps the
  job fast and simple.
- **Query suite:** default.
- **Triggers:**
  - `pull_request` into `main` — scan every PR before merge.
  - `push` to `main` — keep the baseline current after squash-merges.
  - `schedule` (weekly cron) — re-scan unchanged code against newly published
    CodeQL queries.
- **Permissions:** top-level `permissions: {}`; the analyze job opts into
  `security-events: write` (upload alerts), `contents: read` (checkout), and
  `actions: read`.
- **House-style parity:** `concurrency` group with `cancel-in-progress: true`,
  `timeout-minutes`, `actions/checkout` with `persist-credentials: false`, all
  actions pinned consistently with the rest of the repo.
- **Results:** Code scanning alerts in the **Security** tab, with Copilot Autofix
  suggestions on alerts.
- **Dependabot:** no config change required — `codeql-action` is a GitHub Action
  in `/`, already covered by the existing `github-actions` update group, so it is
  version-bumped automatically.

## Component 2 — CodeRabbit

Two parts: an install step the maintainer performs, and an in-repo config file.

### Install (manual, maintainer)

Install the **CodeRabbit GitHub App** from the marketplace and grant it access to
the `NotAProfDev/oath` repository. CodeRabbit Pro is **free forever for public
repos** (full feature set, no seat limit). This step cannot be done from files in
the repo and will be listed as a manual action in the implementation plan.

### Config — `.coderabbit.yaml`

Committed at repo root so CodeRabbit's behavior is reviewable in git like every
other tool here. Starting configuration tuned for a **solo maintainer
(low-noise)**:

- **`chill` review profile** — fewer nitpick comments.
- Enable CodeRabbit's **Rust-aware tooling** (clippy and the linters it bundles)
  and security-relevant linters (e.g. gitleaks/actionlint equivalents) where they
  do not duplicate the existing `just ci` gate.
- **Respect Conventional Commits** for PR titles (matches the `commit-msg` hook
  convention).
- **Path filters** to skip generated/vendored noise (e.g. `target/`,
  `Cargo.lock`).

The exact `.coderabbit.yaml` keys will be finalized against CodeRabbit's current
schema during implementation; this section fixes intent, not field names.

## Out of scope / deferred (recorded for later)

- **Coverage** — cargo-llvm-cov + Codecov (tokenless for public repos). Skipped
  now; the trait-only crates have ~0% meaningful coverage. Revisit when backends
  land.
- **GitHub Code Quality** — evaluate after 2026-07-20 GA; adopt only if it adds
  signal over CodeRabbit + clippy. Built on the CodeQL engine; partial overlap
  with CodeRabbit on maintainability findings.
- **Branch protection** — optionally make the CodeQL check required on `main`
  once it has run green at least once.

## Testing / verification

- `actionlint` (already in `just ci`) must pass on the new `codeql.yml`.
- The CodeQL workflow must complete a successful run on a PR and surface (or
  cleanly report zero) alerts in the Security tab.
- `just ci` remains green and unchanged — neither addition touches the local gate.
- After install, CodeRabbit must post a review on a test PR, and
  `.coderabbit.yaml` must be accepted (no config-parse error in its comment).

## Definition of done

- `.github/workflows/codeql.yml` exists, passes `actionlint`, and runs green.
- `.coderabbit.yaml` exists at repo root.
- CodeRabbit app installed and reviewing PRs (manual step completed).
- This decision — including the deferred items — is recorded in this spec.
- Work follows the one-issue-one-PR lifecycle in CLAUDE.md.
