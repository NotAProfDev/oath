# Pipeline Hardening — CodeQL + CodeRabbit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GitHub CodeQL (Rust security SAST) and CodeRabbit (AI PR review) to the CI pipeline to catch bugs and quality issues before they reach `main`.

**Architecture:** Two new in-repo config files plus one manual app install. `.github/workflows/codeql.yml` runs the CodeQL engine (no-build mode, default query suite) on PRs, pushes to `main`, and a weekly schedule; results land as Code scanning alerts. `.coderabbit.yaml` configures the CodeRabbit GitHub App for low-noise AI review, with its bundled deterministic linters disabled because `just ci` already enforces them.

**Tech Stack:** GitHub Actions, `github/codeql-action@v3` (CodeQL for Rust, GA Oct 2025), CodeRabbit (config schema v2), `actionlint`, `python3`/`pyyaml` for local YAML validation.

## Global Constraints

- **Branch:** `ci/codeql-coderabbit` (already created off `main`); tracked by issue **#23**. One issue, one PR.
- **House style for workflows (copy from `.github/workflows/ci.yml`):** top-level `permissions: {}`; per-job least-privilege `permissions:`; `concurrency` group `${{ github.workflow }}-${{ github.ref }}` with `cancel-in-progress: true`; `timeout-minutes: 30`; `actions/checkout@v7` with `persist-credentials: false`; all actions pinned by major tag, consistent with the existing workflow.
- **Conventional Commits**, enforced by the `commit-msg` hook — use `ci(...)` / `chore(...)` prefixes for commits here.
- **Definition of done:** `just ci` must remain green and unchanged; new workflow must pass `actionlint`; neither file touches the local `just ci` gate's behavior.
- **No `unsafe`/`unwrap`/etc.** — not applicable here (no Rust code is added), but do not modify any Rust source.

---

### Task 1: CodeQL workflow

**Files:**
- Create: `.github/workflows/codeql.yml`
- Verify with: `just actionlint` (runs `actionlint` over `.github/workflows/`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a `CodeQL` workflow with an `analyze` job. No other task depends on its contents.

- [ ] **Step 1: Write the workflow file**

Create `.github/workflows/codeql.yml` with exactly this content:

```yaml
# CodeQL security analysis (SAST) for Rust.
# Full reference: https://docs.github.com/en/code-security/code-scanning
#
# Runs GitHub's CodeQL engine against the workspace to surface security-relevant
# bug patterns (injection, unsafe data flow, crypto misuse, ...). Findings appear
# as Code scanning alerts in the Security tab, with Copilot Autofix suggestions.

name: CodeQL

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]
  # Re-scan unchanged code weekly so newly published CodeQL queries catch latent
  # issues that no PR would otherwise trigger a scan for. Monday 04:30 UTC.
  schedule:
    - cron: "30 4 * * 1"

# Cancel any in-progress run for the same branch when a new push arrives.
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

# Least-privilege floor: grant nothing by default; the job opts into what it needs.
permissions: {}

jobs:
  analyze:
    name: Analyze (rust)
    runs-on: ubuntu-latest
    timeout-minutes: 30
    permissions:
      # Upload analysis results as code scanning alerts.
      security-events: write
      # Read the repo (checkout) and workflow metadata.
      contents: read
      actions: read
    steps:
      - uses: actions/checkout@v7
        with:
          # CodeQL never pushes; don't persist the token in .git/config.
          persist-credentials: false

      # Rust no-build scanning is GA (2025-10-14): CodeQL extracts source directly,
      # so no Rust toolchain setup or cargo build is required. Default query suite
      # (high precision, low false-positive) is used when `queries` is omitted.
      - name: initialize CodeQL
        uses: github/codeql-action/init@v3
        with:
          languages: rust
          build-mode: none

      - name: perform CodeQL analysis
        uses: github/codeql-action/analyze@v3
        with:
          category: "/language:rust"
```

- [ ] **Step 2: Lint the workflow and verify it passes**

Run: `just actionlint`
Expected: PASS — no output / exit code 0. (`actionlint` validates syntax, `on:` triggers, `permissions`, and expression interpolation in the new file.)

If `actionlint` reports an error, fix the reported line and re-run until clean.

- [ ] **Step 3: Confirm the local CI gate is unaffected**

Run: `just ci`
Expected: PASS — identical result to before this change. `just ci` does not run CodeQL (that is GitHub-only), but it does run `actionlint`, so this confirms the new file is accepted by the full gate.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/codeql.yml
git commit -m "ci(codeql): add Rust security scanning workflow"
```

---

### Task 2: CodeRabbit configuration

**Files:**
- Create: `.coderabbit.yaml` (repo root)
- Verify with: `python3 -c "import yaml; yaml.safe_load(open('.coderabbit.yaml'))"`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a CodeRabbit config consumed by the CodeRabbit GitHub App once installed in Task 3.

- [ ] **Step 1: Write the config file**

Create `.coderabbit.yaml` at the repo root with exactly this content. All keys and enum values are taken from CodeRabbit config schema v2.

```yaml
# yaml-language-server: $schema=https://coderabbit.ai/integrations/schema.v2.json
# CodeRabbit configuration — AI pull-request review.
# Schema: https://docs.coderabbit.ai/reference/yaml-template
#
# Tuned for a low-noise, single-maintainer workflow. CodeRabbit's value here is
# the AI review layer; the deterministic linters it bundles (clippy, gitleaks,
# actionlint) are intentionally disabled because `just ci` already enforces them
# at deny-level, and re-reporting them would only duplicate comments.
language: en-US
reviews:
  # Fewer nitpick comments; surface only higher-signal feedback.
  profile: chill
  # Don't gate merges on CodeRabbit; the maintainer decides when to merge.
  request_changes_workflow: false
  high_level_summary: true
  auto_review:
    enabled: true
  # Skip generated / vendored / lockfile noise. `!` prefixes exclude a glob.
  path_filters:
    - "!target/**"
    - "!**/Cargo.lock"
  # Warn (don't block) when a PR title doesn't follow Conventional Commits. PR
  # titles become the squash-merge commit subject, which the local commit-msg
  # hook never sees — this is the only check covering that gap.
  pre_merge_checks:
    title:
      mode: warning
      requirements: >-
        The PR title must follow Conventional Commits
        (e.g. `feat(engine): ...`, `fix(risk): ...`, `ci(actions): ...`,
        `chore(deps): ...`), matching the repo's commit-msg hook.
  tools:
    # All three below are already enforced by `just ci` — disable to avoid
    # duplicate comments and keep CodeRabbit focused on AI review.
    clippy:
      enabled: false
    gitleaks:
      enabled: false
    actionlint:
      enabled: false
```

- [ ] **Step 2: Validate YAML syntax and verify it parses**

Run: `python3 -c "import yaml; yaml.safe_load(open('.coderabbit.yaml')); print('yaml ok')"`
Expected: prints `yaml ok` (exit code 0). If it raises a `yaml.YAMLError`, fix the indentation/quoting at the reported line and re-run.

- [ ] **Step 3: Confirm the local CI gate is unaffected**

Run: `just ci`
Expected: PASS. `just ci` runs `typos` across all files (including `.coderabbit.yaml`); this confirms the new file introduces no spelling-gate failures and the gate stays green.

- [ ] **Step 4: Commit**

```bash
git add .coderabbit.yaml
git commit -m "ci(coderabbit): add AI review config tuned for low-noise solo review"
```

---

### Task 3: Manual install, PR, and follow-ups

This task has no code. It installs the CodeRabbit app (cannot be done from repo files), opens the PR, and records the deferred follow-ups. The maintainer (`@NotAProfDev`) performs the install steps.

**Files:**
- None created/modified.

**Interfaces:**
- Consumes: the committed `.coderabbit.yaml` (Task 2) and `.github/workflows/codeql.yml` (Task 1).
- Produces: a merged PR closing issue #23; CodeRabbit active on the repo.

- [ ] **Step 1: Push the branch**

```bash
git push -u origin ci/codeql-coderabbit
```
Expected: branch pushed; the `ci.yml` and new `codeql.yml` workflows start on the push.

- [ ] **Step 2: Open the PR referencing the issue**

```bash
gh pr create \
  --base main \
  --head ci/codeql-coderabbit \
  --title "ci: add CodeQL + CodeRabbit for earlier bug/quality detection" \
  --body "Closes #23.

Adds CodeQL Rust security scanning (\`.github/workflows/codeql.yml\`) and
CodeRabbit AI review config (\`.coderabbit.yaml\`). Design:
docs/superpowers/specs/2026-06-22-pipeline-hardening-codeql-coderabbit-design.md"
```
Expected: PR URL printed. The PR's `Closes #23` will auto-close the issue on merge.

- [ ] **Step 3: Install the CodeRabbit GitHub App (maintainer, browser)**

1. Go to https://github.com/marketplace/coderabbitai and choose the **Free / Open Source** plan ($0 for public repos).
2. Install / configure the app and grant it access to the **`NotAProfDev/oath`** repository only.
3. CodeRabbit will detect `.coderabbit.yaml` automatically on the next review.

Expected: CodeRabbit posts a review (summary + any inline comments) on the open PR. If it posts a config-parse error comment, fix the reported key in `.coderabbit.yaml`, commit, and push.

- [ ] **Step 4: Verify both tools ran on the PR**

Confirm on the PR / repo:
- The **CodeQL** check appears in the PR checks and completes (green, or with alerts shown in the **Security → Code scanning** tab).
- **CodeRabbit** has posted its review and did **not** report a `.coderabbit.yaml` parse error.
- The existing **CI** and **MSRV** checks are still green.

Expected: all of the above true.

- [ ] **Step 5: Squash-merge after cloud CI is green**

Once all required checks pass and review is addressed:
```bash
gh pr merge --squash --delete-branch
```
Verify the squash-merge **title** follows Conventional Commits (CodeRabbit's title check warns if not). Merging closes issue #23.

- [ ] **Step 6: Record deferred follow-ups (optional, post-merge)**

These are documented in the spec; act on them later, not now:
- **Branch protection:** in repo Settings → Branches/Rulesets, optionally add **CodeQL** as a required status check on `main` (do this only after CodeQL has run green at least once).
- **Coverage:** revisit `cargo-llvm-cov` + Codecov when backend crates carry real logic.
- **GitHub Code Quality:** evaluate after its 2026-07-20 GA; adopt only if it adds signal over CodeRabbit + clippy.

---

## Self-Review

**1. Spec coverage:**
- CodeQL workflow (engine, build-mode none, default suite, PR + push + weekly schedule, permissions, house-style parity) → **Task 1**. ✓
- Dependabot needs no change (codeql-action covered by existing github-actions group) → noted in spec; no task needed (correct — nothing to do). ✓
- CodeRabbit install (manual) → **Task 3 Step 3**. ✓
- CodeRabbit `.coderabbit.yaml` (chill profile, Rust tooling decision, Conventional-Commit title check, path filters) → **Task 2**. ✓ Note: spec said "enable Rust-aware tooling where it does not duplicate the gate"; clippy/gitleaks/actionlint *do* duplicate `just ci`, so they are disabled — consistent with the "do not duplicate" clause and the low-noise goal.
- Deferred items (coverage, Code Quality, branch protection) → **Task 3 Step 6** + spec. ✓
- Testing/verification (actionlint green, CodeQL runs, just ci green, CodeRabbit accepts config) → Tasks 1–3 verification steps. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". Both file contents are complete and literal. ✓

**3. Type/key consistency:** CodeRabbit keys (`reviews.profile`, `reviews.request_changes_workflow`, `reviews.high_level_summary`, `reviews.auto_review.enabled`, `reviews.path_filters`, `reviews.pre_merge_checks.title.mode`, `reviews.tools.<tool>.enabled`) match schema v2 verified values. CodeQL action refs (`github/codeql-action/init@v3`, `.../analyze@v3`, `languages: rust`, `build-mode: none`, `category: "/language:rust"`) are consistent across the task. Branch name `ci/codeql-coderabbit` and issue `#23` consistent throughout. ✓
