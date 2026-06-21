# .github Folder Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct the factual inconsistencies in OATH's `.github/` folder, harden the CI workflow, and add the three high-value governance files (SECURITY, CONTRIBUTING, CODEOWNERS) — scaled to a pre-release, solo-maintainer project.

**Architecture:** Pure configuration/documentation work — no Rust code changes. Each task touches one file (or one tightly-coupled pair), is independently committable, and is verified by an existing `just` recipe or a YAML/`git` check. Source spec: [docs/superpowers/specs/2026-06-21-github-folder-alignment-design.md](../specs/2026-06-21-github-folder-alignment-design.md).

**Tech Stack:** GitHub Actions, Dependabot, GitHub issue forms (YAML), Markdown, `just` task runner, `actionlint`.

## Global Constraints

- **No new CI workflows.** Harden-only: no CodeQL, OpenSSF Scorecard, or `dependency-review-action`. (spec §"CI/security")
- **No email anywhere.** SECURITY.md uses GitHub private vulnerability reporting only. (spec §"SECURITY.md reporting channel")
- **Conventional Commits**, enforced by the `commit-msg` hook. Types: `feat fix docs style refactor perf test build ci chore revert`. Subject ≤ 72 chars.
- **Don't bypass git hooks.** Every commit runs `just pre-commit` (check-branch, fmt, fmt-toml, typos, lint, test-no-run). Commits must be on a feature branch, never `main`.
- **Leave all GitHub Action versions unchanged** — Dependabot owns them. `dtolnay/rust-toolchain@stable` / `@1.85` stay as-is.
- **British spelling** is used in existing templates (`behaviour`, `honours`); keep `typos` green.
- Governance files live in `.github/`; only `.gitignore` (root) is touched outside it.

---

### Task 1: Track Cargo.lock and correct the Dependabot comment

The `.gitignore` entry for `Cargo.lock` is the root cause of the false comment in `dependabot.yml`. `Cargo.lock` is already committed and every CI step runs `--locked`, so the ignore entry is simply wrong. Fix both together.

**Files:**
- Modify: `.gitignore` (remove the `Cargo.lock` block, lines 13–15)
- Modify: `.github/dependabot.yml` (header comment + cargo ecosystem comment)

**Interfaces:**
- Consumes: nothing.
- Produces: a truthful Dependabot rationale that later documentation (CONTRIBUTING) can rely on.

- [ ] **Step 1: Remove the `Cargo.lock` block from `.gitignore`**

Delete these three lines (currently lines 13–15):

```gitignore
# Remove Cargo.lock from gitignore if creating an executable, leave it for libraries
# More information here https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html
Cargo.lock
```

- [ ] **Step 2: Verify `Cargo.lock` is no longer ignored**

Run: `git check-ignore Cargo.lock; echo "exit=$?"`
Expected: no path printed, `exit=1` (meaning the file is NOT ignored).

- [ ] **Step 3: Fix the header comment in `.github/dependabot.yml`**

Replace this block:

```yaml
# Opens weekly PRs for outdated dependencies across all three package ecosystems.
# Cargo.lock is gitignored (library convention), so Dependabot targets Cargo.toml
# version constraints directly.
```

with:

```yaml
# Opens weekly PRs for outdated dependencies across all three package ecosystems.
# Cargo.lock is committed and every CI/just recipe runs with `--locked`, so the
# cargo updates refresh both the Cargo.toml constraints and the Cargo.lock entries.
```

- [ ] **Step 4: Fix the inline cargo-ecosystem comment in `.github/dependabot.yml`**

Replace:

```yaml
  # Rust crate dependencies — targets version constraints in Cargo.toml files.
```

with:

```yaml
  # Rust crate dependencies — updates Cargo.toml constraints and the committed Cargo.lock.
```

- [ ] **Step 5: Verify `dependabot.yml` is still valid YAML**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/dependabot.yml')); print('ok')"`
Expected: `ok`. (If `python3` is unavailable, instead run `git diff .github/dependabot.yml` and confirm only the two comment blocks changed.)

- [ ] **Step 6: Commit**

```bash
git add .gitignore .github/dependabot.yml
git commit -m "build: track Cargo.lock and correct dependabot lockfile comment"
```

---

### Task 2: Harden the CI workflow

Add a least-privilege `permissions` floor, drop checkout credentials, add job timeouts, fix the misleading permissions comment, and tidy comment indentation. No version or gate-composition changes.

**Files:**
- Modify: `.github/workflows/ci.yml` (replace whole file)

**Interfaces:**
- Consumes: nothing.
- Produces: a hardened workflow that still maps 1:1 to `just ci` + `just msrv`.

- [ ] **Step 1: Replace `.github/workflows/ci.yml` with the hardened version**

```yaml
# CI workflow — runs on every push to main and on every pull request.
# Full reference: https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions

name: CI

on:
  push:
    branches: [main]
  pull_request:

# Cancel any in-progress run for the same branch when a new push arrives,
# avoiding redundant work on fast-moving PR branches.
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

# Least-privilege floor for the whole workflow: grant nothing by default and let
# each job opt into exactly the scopes it needs.
permissions: {}

jobs:
  ci:
    name: CI
    runs-on: ubuntu-latest
    timeout-minutes: 30
    # checkout is the only step that uses GITHUB_TOKEN, and it only reads the
    # repo. The cache and artifact-upload steps authenticate with the Actions
    # runtime token (ACTIONS_RUNTIME_TOKEN), not GITHUB_TOKEN — so contents:read
    # is the complete set of scopes this job needs.
    permissions:
      contents: read
    env:
      # Preserve ANSI colour codes in cargo output for readable Actions logs.
      CARGO_TERM_COLOR: always
    steps:
      - uses: actions/checkout@v6
        with:
          # CI never pushes; don't persist the token in .git/config.
          persist-credentials: false

      # rust-toolchain.toml supplies channel/components; this action honours it.
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      # Cache registry/git/target keyed by OS + Rust version + Cargo.lock.
      - uses: Swatinem/rust-cache@v2

      # Single fast installer for every non-rustc tool the just recipes call.
      # If a tool name errors, check the supported list at
      # https://github.com/taiki-e/install-action and adjust that one name.
      - uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest,cargo-deny,cargo-machete,taplo-cli,typos-cli,gitleaks,actionlint,shellcheck

      # One command — identical to `just ci` run locally and by the pre-push hook.
      - name: ci
        run: just ci

      # `just ci` runs `cargo doc`; publish the generated HTML.
      - name: upload docs
        uses: actions/upload-artifact@v7
        with:
          name: docs
          path: target/doc
          retention-days: 7

  # Verify that the codebase compiles on the declared minimum supported Rust
  # version. rust-version = "1.85" in Cargo.toml is metadata only — without
  # this job, accidentally using a post-1.85 API would go undetected until a
  # downstream user reports a compile error.
  msrv:
    name: MSRV (1.85)
    runs-on: ubuntu-latest
    timeout-minutes: 30
    permissions:
      contents: read
    env:
      CARGO_TERM_COLOR: always
    steps:
      - uses: actions/checkout@v6
        with:
          persist-credentials: false

      - uses: dtolnay/rust-toolchain@1.85

      # Cache keyed separately from the `ci` job because of the different toolchain.
      - uses: Swatinem/rust-cache@v2
        with:
          key: msrv

      - uses: taiki-e/install-action@v2
        with:
          tool: just

      - name: msrv
        run: just msrv
```

- [ ] **Step 2: Verify the workflow lints clean**

Run: `just actionlint`
Expected: no output, exit 0.

- [ ] **Step 3: Confirm the gate composition is unchanged**

Run: `git diff .github/workflows/ci.yml | grep -E '^\+' | grep -iE 'tool:|run:'`
Expected: the `tool:` install line and the `run: just ci` / `run: just msrv` lines are byte-identical to before (only comments, `permissions`, `timeout-minutes`, and `persist-credentials` were added).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(actions): add least-privilege permissions and harden CI workflow"
```

---

### Task 3: Fix the feature-request issue template

Correct the dead plan-path convention, the "PRD" wording, and the crate-name shorthand.

**Files:**
- Modify: `.github/ISSUE_TEMPLATE/feature_request.yml`

**Interfaces:**
- Consumes: the real plan convention `docs/superpowers/plans/<slug>.md` (Task in this repo's `docs/` tree).
- Produces: nothing downstream depends on it.

- [ ] **Step 1: Fix the Implementation Plan field**

Replace:

```yaml
      description: Link to the implementation plan at .claude/plans/<slug>.md (created by /plan-feature). If not yet created, leave blank.
      placeholder: ".claude/plans/order-lifecycle.md"
```

with:

```yaml
      description: Link to the implementation plan at docs/superpowers/plans/<slug>.md (created with the writing-plans skill). If not yet created, leave blank.
      placeholder: "docs/superpowers/plans/order-lifecycle.md"
```

- [ ] **Step 2: Fix the "In scope" wording**

Replace:

```yaml
      label: In scope
      description: What this PRD explicitly covers.
```

with:

```yaml
      label: In scope
      description: What this feature explicitly covers.
```

- [ ] **Step 3: Align the affected-crates description to real crate names**

Replace:

```yaml
      description: Which OATH crates will this feature touch? (model, net/core, messaging/core, persistence/core, ingest/core, execution/core, portfolio/core, risk/core, strategy/core, engine)
```

with:

```yaml
      description: Which OATH crates will this feature touch? (oath-model, oath-net-core, oath-messaging-core, oath-persistence-core, oath-ingest-core, oath-execution-core, oath-portfolio-core, oath-risk-core, oath-strategy-core, oath-engine)
```

- [ ] **Step 4: Verify the template is still valid YAML**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/ISSUE_TEMPLATE/feature_request.yml')); print('ok')"`
Expected: `ok`. (If `python3` is unavailable, run `git diff .github/ISSUE_TEMPLATE/feature_request.yml` and confirm only the three string values changed — no indentation or key changes.)

- [ ] **Step 5: Commit**

```bash
git add .github/ISSUE_TEMPLATE/feature_request.yml
git commit -m "docs(issue-template): fix plan path, PRD wording, and crate names"
```

---

### Task 4: Add SECURITY.md

GitHub private vulnerability reporting only; pre-release supported-versions note; no email.

**Files:**
- Create: `.github/SECURITY.md`

**Interfaces:**
- Consumes: nothing.
- Produces: `.github/SECURITY.md`, linked from CONTRIBUTING.md (Task 5).

- [ ] **Step 1: Create `.github/SECURITY.md`**

```markdown
# Security Policy

> ⚠️ OATH is **pre-release software — do not use it in production.** No versions
> have been released yet, and the API is unstable.

## Supported versions

Only the `main` branch is supported. There are no released versions, so security
fixes land on `main` and are not backported.

| Version | Supported          |
| ------- | ------------------ |
| `main`  | :white_check_mark: |
| Released versions | None yet |

## Reporting a vulnerability

Please report security vulnerabilities through GitHub's **private vulnerability
reporting**:

1. Open the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Fill in the advisory form with as much detail as you can — affected crate,
   reproduction steps, and impact.

This keeps the report private until a fix is ready. Please do **not** open a
public issue for security-sensitive reports.

## What to expect

OATH is maintained by a single person on a best-effort basis, so response times
vary. You can expect an acknowledgement once the report has been read, and
coordination through the private advisory thread until the issue is resolved or
declined. There is no formal SLA at this stage.
```

- [ ] **Step 2: Verify the file renders as Markdown (no broken table)**

Run: `grep -c '|' .github/SECURITY.md`
Expected: `4` (three table rows × the pipe-delimited columns confirm the table block is intact).

- [ ] **Step 3: Commit**

```bash
git add .github/SECURITY.md
git commit -m "docs: add security policy"
```

---

### Task 5: Add CONTRIBUTING.md

A summary of CLAUDE.md that points back to it as the source of truth — not a fork.

**Files:**
- Create: `.github/CONTRIBUTING.md`

**Interfaces:**
- Consumes: `.github/SECURITY.md` (Task 4), `.github/ISSUE_TEMPLATE/`, `.github/PULL_REQUEST_TEMPLATE.md`, `../CLAUDE.md`, `../README.md`.
- Produces: `.github/CONTRIBUTING.md`, linked from the PR template (Task 6).

- [ ] **Step 1: Create `.github/CONTRIBUTING.md`**

Note: links are relative to `.github/`, so root files use `../`.

```markdown
# Contributing to OATH

Thanks for your interest in OATH! This guide summarises how we work. For the
authoritative, always-current details, see [CLAUDE.md](../CLAUDE.md) and
[README.md](../README.md) — this document points back to them rather than
duplicating them.

> ⚠️ OATH is **pre-release — do not use in production.** It is still being built.

## Development workflow — one issue, one PR

1. **Issue** — open a GitHub issue using the [issue templates](ISSUE_TEMPLATE)
   (bug, feature, or question).
2. **Branch** — branch off `main` for that one issue: `feat/<slug>` or
   `fix/<slug>`. One branch implements one issue.
3. **Implement** — make the change. For non-trivial work, write a spec then a
   plan first (under `../docs/superpowers/`).
4. **Local CI** — `just ci` must pass. The pre-push hook enforces the full gate,
   so a clean push means local CI is green.
5. **Pull request** — open a PR that references the issue (`Closes #N`) and fill
   in the [PR template](PULL_REQUEST_TEMPLATE.md).
6. **Cloud CI** — GitHub Actions runs `just ci` plus the MSRV job; it must be
   green to merge.
7. **Squash + merge** — the PR is squash-merged into `main`; the issue closes.

## Getting set up

- **Dev container (recommended):** open the repo in the dev container; it
  provisions all tooling (`gh`, `just`, the `cargo-*` tools, `gitleaks`,
  `shellcheck`, `actionlint`, `typos`, `taplo`) and wires the git hooks for you.
- **Local clone:** run `just setup` once to point Git at `.githooks` and make the
  hooks executable. Run `just --list` to see every recipe.

## Commands

`just` is the single entry point — prefer a recipe over raw `cargo`, because the
recipes pin the exact flags CI uses.

| Task | Command |
|---|---|
| Type-check everything | `just check` |
| Run tests (+ doctests) | `just test` |
| Lint (warnings = errors) | `just lint` |
| Auto-fix fmt + clippy | `just fix` |
| Full local CI suite | `just ci` |

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org), enforced by
the `commit-msg` hook — for example `feat(engine): …`, `fix(risk): …`,
`chore(deps): …`, `ci(actions): …`. Subjects are capped at 72 characters.

## Code conventions

Hard rules, enforced workspace-wide by `[workspace.lints]` (see
[CLAUDE.md](../CLAUDE.md) for the full list):

- **No `unsafe`** — FFI needs a per-crate override with justification.
- **No `unwrap` / `expect` / indexing** in non-test code — return `Result` and
  model errors with `thiserror`. Test code is exempt.
- **Document public items** — `missing_docs` is on.
- **Respect the dependency direction** in the README graph — never introduce a
  cycle.
- Edition **2024**, MSRV **1.85** (validate with `just msrv`).

Work is not done until `just ci` passes — identical to the cloud CI gate. Don't
bypass the git hooks.

## Security

Found a vulnerability? Please follow [SECURITY.md](SECURITY.md) — report it
through GitHub private vulnerability reporting, not a public issue.
```

- [ ] **Step 2: Verify the relative links point at real targets**

Run:
```bash
for p in CLAUDE.md README.md .github/ISSUE_TEMPLATE .github/PULL_REQUEST_TEMPLATE.md .github/SECURITY.md; do
  test -e "$p" && echo "ok: $p" || echo "MISSING: $p"
done
```
Expected: five `ok:` lines, no `MISSING:`.

- [ ] **Step 3: Commit**

```bash
git add .github/CONTRIBUTING.md
git commit -m "docs: add contributing guide"
```

---

### Task 6: Link CONTRIBUTING from the PR template

**Files:**
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`

**Interfaces:**
- Consumes: `.github/CONTRIBUTING.md` (Task 5).
- Produces: nothing downstream.

- [ ] **Step 1: Add a pointer line at the very top of `.github/PULL_REQUEST_TEMPLATE.md`**

Insert as the first line, before `## Description`:

```markdown
<!-- New here? See CONTRIBUTING.md for the issue → branch → just ci → PR → squash workflow. -->

```

(The existing `## Description` heading and the rest of the file stay unchanged.)

- [ ] **Step 2: Verify the pointer is present and the structure intact**

Run: `head -n 1 .github/PULL_REQUEST_TEMPLATE.md; grep -c '^## ' .github/PULL_REQUEST_TEMPLATE.md`
Expected: the comment line prints first, and the heading count is `4` (Description, Related Issue, Changes, Checklist).

- [ ] **Step 3: Commit**

```bash
git add .github/PULL_REQUEST_TEMPLATE.md
git commit -m "docs: link contributing guide from the PR template"
```

---

### Task 7: Add CODEOWNERS

**Files:**
- Create: `.github/CODEOWNERS`

**Interfaces:**
- Consumes: nothing.
- Produces: automatic review-request routing.

- [ ] **Step 1: Create `.github/CODEOWNERS`**

```
# Code owners for OATH. Each pattern maps to its reviewer(s); GitHub auto-requests
# review from the owner when a matching file changes in a pull request.
# Reference: https://docs.github.com/en/repositories/managing-your-repositories-settings-and-features/customizing-your-repository/about-code-owners

# Default owner for everything in the repository.
* @NotAProfDev
```

- [ ] **Step 2: Verify the default rule is present**

Run: `grep -E '^\* @NotAProfDev$' .github/CODEOWNERS`
Expected: the line `* @NotAProfDev` prints.

- [ ] **Step 3: Commit**

```bash
git add .github/CODEOWNERS
git commit -m "chore: add CODEOWNERS"
```

---

### Task 8: Final full-gate verification

Confirm the whole repository still passes the complete CI gate after all edits.

**Files:** none (verification only).

- [ ] **Step 1: Run the full local CI suite**

Run: `just ci`
Expected: every step (fmt, fmt-toml, typos, lint, check, test, deny, doc, machete, gitleaks, actionlint, shellcheck) passes, exit 0. `typos` in particular must be clean across the two new Markdown files.

- [ ] **Step 2: Confirm the working tree is clean**

Run: `git status --short`
Expected: no output (all changes committed across Tasks 1–7).

- [ ] **Step 3: Review the full set of changes**

Run: `git log --oneline -8`
Expected: the seven task commits (Tasks 1–7) plus the spec commit, in order.

- [ ] **Step 4: Sanity-check the Discussions link (manual)**

Open the URL in `.github/ISSUE_TEMPLATE/config.yml`
(`https://github.com/NotAProfDev/oath/discussions`) in a browser, or run
`gh api repos/NotAProfDev/oath --jq .has_discussions`.
Expected: the Discussions tab exists (`true`). If Discussions is disabled,
enable it in repo settings or remove the `contact_links` entry — note this is a
repo-settings action, not a code change.
```
