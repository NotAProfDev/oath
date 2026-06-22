# Design: `.github` folder review & alignment

**Date:** 2026-06-21
**Status:** Approved (pending implementation plan)
**Topic:** Audit the `.github/` folder, correct inconsistencies against the
actual OATH workflow, and add the high-value missing community/governance files.

## Goal

Bring everything under `.github/` into line with (a) the real OATH development
workflow described in [CLAUDE.md](../../../CLAUDE.md) and the `justfile`, (b)
GitHub-platform best practices, and (c) sensible industry standards **scaled to a
pre-release, single-maintainer project**. Fix what is factually wrong, harden the
CI workflow without adding new pipelines, and add the three governance files that
earn their keep at this stage.

## Scope decisions (locked with the user)

- **Files:** fix the 7 existing files **and** add `SECURITY.md`,
  `CONTRIBUTING.md`, `CODEOWNERS`. Skip `CODE_OF_CONDUCT.md`, `FUNDING.yml`,
  `SUPPORT.md` for now (premature ceremony for a solo pre-release repo).
- **CI/security:** **harden existing only** — no new workflows. Specifically no
  CodeQL, no OpenSSF Scorecard, no `dependency-review-action`.
- **SECURITY.md reporting channel:** GitHub **private vulnerability reporting**
  only. Do **not** publish the maintainer's email anywhere.
- **`.gitignore` fix (root):** approved to include even though it sits outside
  `.github/`, because the Dependabot comment's correctness depends on it.

## Findings that drive the work

1. **`dependabot.yml` comment is false.** It states "Cargo.lock is gitignored
   (library convention), so Dependabot targets Cargo.toml version constraints
   directly." In reality `Cargo.lock` **is tracked** (`git ls-files` confirms),
   and the `justfile` runs every cargo step with `--locked`, which *requires* a
   committed, up-to-date lockfile. The `Cargo.lock` entry in `.gitignore` is the
   actual bug — it never untracked the already-committed file, it just misstates
   intent.
2. **`feature_request.yml` references a dead convention.** The "Implementation
   Plan" field points to `.claude/plans/<slug>.md` created by `/plan-feature`.
   The real convention (CLAUDE.md + the `docs/` tree) is
   `docs/superpowers/plans/<slug>.md`, produced by the `writing-plans` skill.
3. **`ci.yml` permissions comment overclaims.** The comment says the job needs to
   "write to Actions (cache + artifact upload)", but cache and artifact upload use
   the Actions **runtime** token (`ACTIONS_RUNTIME_TOKEN`), not `GITHUB_TOKEN`
   scopes. The job correctly grants only `contents: read`; the comment is wrong,
   and there is no workflow-level `permissions` floor.
4. **Missing standard files:** `SECURITY.md`, `CONTRIBUTING.md`, `CODEOWNERS`
   (the three chosen in scope) are absent.
5. **Minor wording drift:** `feature_request.yml` calls the In-scope field a
   "PRD"; its `affected-crates` placeholder uses `net/core` shorthand instead of
   the real crate names (`oath-net-core`, …) from the README.

## The work, by file

### A. Corrections to existing files

#### `.github/workflows/ci.yml` (harden-only)

- Add a workflow-level least-privilege floor: top-level `permissions: {}`,
  retaining each job's explicit `permissions: contents: read`.
- Rewrite the misleading permissions comment so it states the truth: the job only
  needs `contents: read` for checkout; cache/artifact upload use the runtime
  token, not `GITHUB_TOKEN` scopes.
- Add `with: persist-credentials: false` to both `actions/checkout` steps (CI
  never pushes — drop the credential from the checkout).
- Add `timeout-minutes` to both `ci` and `msrv` jobs (runaway-runner guard;
  pick a generous bound, e.g. 30).
- Tidy the comment blocks that are wrongly indented under prior steps' `with:` keys.
- Leave all action versions unchanged (Dependabot owns them); only confirm none
  are yanked. `dtolnay/rust-toolchain@stable`/`@1.85` stay as-is (the `@1.85`
  pin is the intentional MSRV and is already excluded in `dependabot.yml`).
- Must still pass `actionlint` (run via `just actionlint`).

#### `.github/dependabot.yml`

- Replace the false `Cargo.lock` comment with an accurate one: the lockfile is
  committed and CI runs `--locked`, so Dependabot keeps both `Cargo.toml` and
  `Cargo.lock` current.
- Keep the three ecosystems (cargo, github-actions, devcontainers), weekly
  cadence, `groups`, the `dtolnay/rust-toolchain` ignore, and the
  `dependencies`/`ci` labels (these already match CLAUDE.md). No structural
  change.

#### `.github/ISSUE_TEMPLATE/feature_request.yml`

- Fix the Implementation Plan field: path `docs/superpowers/plans/<slug>.md`,
  and reference the `writing-plans` skill instead of the nonexistent
  `/plan-feature` command.
- Reword the "In scope" field description: "this **PRD**" → "this feature".
- Update the `affected-crates` placeholder to real crate names from the README
  (e.g. `oath-model, oath-execution-core`).

#### `.github/PULL_REQUEST_TEMPLATE.md`

- Add a one-line pointer to `CONTRIBUTING.md`. Keep the existing structure
  (Description / Related Issue `Closes #` / Changes / Checklist) — it already
  matches the workflow.

#### `.github/ISSUE_TEMPLATE/bug_report.yml`, `question.yml`, `config.yml`

- No structural changes. Verify the Discussions URL in `config.yml` resolves.
  These already align with the labels and solo-maintainer assignee model.

### B. New files (placed in `.github/`)

#### `.github/SECURITY.md`

- **Supported versions:** pre-release — only `main` is supported; no released
  versions yet. State plainly that the project is not production-ready (mirror
  the README "do not use" notice).
- **Reporting:** GitHub private vulnerability reporting (Security → Report a
  vulnerability) as the sole channel. No email address anywhere.
- **Expectations:** brief, honest acknowledgement/response note appropriate for a
  single maintainer (no hard SLA promises).

#### `.github/CONTRIBUTING.md`
Human-facing companion to CLAUDE.md. Mirrors, not duplicates, the canonical
workflow:

- The one-issue-one-PR lifecycle: issue (use templates) → branch off `main`
  (`feat/<slug>`, `fix/<slug>`) → implement → `just ci` green → PR referencing
  `Closes #N` → squash-merge.
- Conventional Commits (enforced by the `commit-msg` hook).
- The hard code rules (no `unsafe`, no `unwrap`/`expect`/indexing in non-test
  code, document public items, edition 2024 / MSRV 1.85).
- Onboarding: devcontainer provides all tooling; non-devcontainer clones run
  `just setup` to wire `core.hooksPath`. `just --list` for recipes.
- Link to the issue templates and `SECURITY.md`.

#### `.github/CODEOWNERS`

- Single rule: `* @NotAProfDev`. Enables automatic review-request routing;
  trivially extended when collaborators join.

### C. Related root-level fix

#### `.gitignore`

- Remove the `Cargo.lock` line (and its now-incorrect explanatory comment about
  libraries). The file is tracked and CI depends on it via `--locked`. This is
  the root cause of the false Dependabot comment in A.

## Out of scope (explicit non-goals)

- `CODE_OF_CONDUCT.md`, `FUNDING.yml`, `SUPPORT.md`.
- Any new CI/security workflow: CodeQL, OpenSSF Scorecard,
  `dependency-review-action`, release automation, `cargo publish`.
- Changing action versions, the CI gate composition, or the `justfile`.
- Restructuring the issue/PR templates beyond the wording fixes above.

## Verification

- `just actionlint` passes after the `ci.yml` edits.
- `just ci` remains green (no gate composition change; the workflow still maps
  1:1 to `just ci`).
- Issue/PR templates render correctly on GitHub (YAML form schema valid).
- GitHub "Community Standards" page reflects the newly added
  SECURITY/CONTRIBUTING files.
- `git check-ignore Cargo.lock` returns nothing after the `.gitignore` edit.

## Risks / notes

- The new files (B) and `.gitignore` (C) touch outside the strict `.github/`
  boundary by one file each (`CONTRIBUTING`/`SECURITY`/`CODEOWNERS` live in
  `.github/`; only `.gitignore` is at root) — explicitly approved.
- CONTRIBUTING.md must stay a *summary* of CLAUDE.md, not a fork of it, to avoid
  drift. It should point at CLAUDE.md/README as the source of truth where detail
  is needed.
