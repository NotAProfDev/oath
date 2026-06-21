# Coding Standards Review & Quality-Gate Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Justfile the single source of truth for every quality gate, wire all installed-but-unused tools into local/hook/CI runs, and document each config's alignment with 2026 industry standards.

**Architecture:** All checks are defined once as `just` recipes. Git hooks are thin wrappers that `exec just <recipe>`; CI installs tooling and runs `just ci`. A web-researched audit (Phase 0) precedes and justifies the config changes.

**Tech Stack:** just, cargo (fmt/clippy/check/doc), cargo-nextest, cargo-deny, cargo-machete, taplo, typos, gitleaks, actionlint, shellcheck, GitHub Actions, Bash.

## Global Constraints

- Rust edition: `2024`; MSRV: `1.85`; license: `MIT OR Apache-2.0` (copy verbatim into any new metadata).
- Toolchain: floating `stable` (no version pin); rustfmt stays stable-only (no nightly options).
- Large-file commit threshold: **5 MiB** (5242880 bytes).
- Conventional Commit types (the only accepted set): `feat fix docs style refactor perf test build ci chore revert`.
- Single source of truth: no quality-gate command may be duplicated between CI and the Justfile. CI calls `just`; hooks call `just`.
- All Bash recipes/scripts use `#!/usr/bin/env bash` and `set -euo pipefail`.
- Commit messages must themselves follow Conventional Commits and end with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- Work happens on the existing `feat/githooks-improvements` branch; do not commit to `main`.

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `docs/superpowers/specs/2026-06-21-coding-standards-audit.md` | Create | Phase 0 gap-analysis matrix with citations |
| `.clippy.toml` | Modify | Add `msrv` |
| `Cargo.toml` | Modify | Add `keywords`/`categories`/`readme` to `[workspace.package]` |
| `Justfile` | Modify | All new recipes + aggregates; switch `test` to nextest |
| `.githooks/pre-commit` | Create | `exec just pre-commit` |
| `.githooks/commit-msg` | Create | `exec just commit-msg "$1"` |
| `.githooks/pre-push` | Create | `exec just pre-push` |
| `.github/workflows/ci.yml` | Modify | Install tools, run `just ci`; `msrv` job runs `just msrv` |

---

### Task 1: Phase 0 — Standards audit

**Files:**
- Create: `docs/superpowers/specs/2026-06-21-coding-standards-audit.md`

**Interfaces:**
- Consumes: nothing.
- Produces: documented rationale + a verdict for each setting. Task 8's config changes must trace to a "gap" or "intentional deviation" row here.

This task is research, not code. No TDD cycle; the deliverable is the completed matrix.

- [ ] **Step 1: Gather current upstream guidance**

Use WebSearch/WebFetch to read the *current* (2026) recommendations for each tool. Authoritative sources to consult (find the live URLs; versions may have moved):
- Rust Style Guide + rustfmt `Configurations.md`
- Clippy configuration / lint-group docs
- The Cargo Book: workspaces, lints table, profiles; Rust API Guidelines (metadata)
- cargo-deny book (v2 config schema)
- rustup overrides (toolchain file) docs
- typos reference
- just manual; Conventional Commits 1.0.0 spec; GitHub Actions security-hardening guide

- [ ] **Step 2: Write the audit document**

Create `docs/superpowers/specs/2026-06-21-coding-standards-audit.md` with this exact skeleton, one matrix per file, every row filled (no blank cells):

```markdown
# Coding Standards Audit — 2026-06-21

Benchmarks each repo config against current upstream standards. Verdicts:
**aligned** | **gap** (will fix in plan Task 8) | **intentional deviation** (documented, kept).

## rustfmt.toml
| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| edition | 2024 | match workspace edition | aligned | — | <url> |
| (nightly import grouping) | disabled | enabled (nightly) | intentional deviation | stay stable-only per project decision | <url> |
| ... | | | | | |

## .clippy.toml
| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
...

## Cargo.toml (lints / profiles / metadata)
...

## deny.toml
...

## rust-toolchain.toml
| channel | "stable" (floating) | pin exact version for reproducibility | intentional deviation | track latest stable per project decision | <url> |
...

## _typos.toml
...

## Justfile / CI / git hooks
...

## Summary
- Gaps to fix in Task 8: <list>
- Intentional deviations (kept): floating stable toolchain; stable-only rustfmt; <others>
```

Every file from the File Structure benchmark list must have a section. Mark the floating-stable toolchain and stable-only rustfmt explicitly as **intentional deviation**, never silently.

- [ ] **Step 3: Verify completeness**

Run: `grep -c '^##' docs/superpowers/specs/2026-06-21-coding-standards-audit.md`
Expected: at least `7` section headers (one per benchmarked file group) plus Summary. No empty table cells; no `<url>` placeholders left unfilled.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-06-21-coding-standards-audit.md
git commit -m "docs: add coding-standards audit matrix vs 2026 industry standards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Justfile atomic tool recipes

**Files:**
- Modify: `Justfile`

**Interfaces:**
- Consumes: nothing.
- Produces: recipes `fmt-toml`, `typos`, `machete`, `gitleaks`, `actionlint`, `test-no-run`; updated `test`. Tasks 6/7 (aggregates, CI) call these by name.

- [ ] **Step 1: Add the new tool recipes**

In `Justfile`, after the existing `fmt` recipe, add:

```just
# Reject any TOML formatting divergence. Fix with: taplo fmt
fmt-toml:
    taplo fmt --check

# Spell-check the repository (config: _typos.toml).
typos:
    typos

# Detect dependencies declared in Cargo.toml but never used.
machete:
    cargo machete

# Scan the worktree and history for committed secrets.
gitleaks:
    gitleaks detect --no-banner

# Lint GitHub Actions workflow files.
actionlint:
    actionlint
```

- [ ] **Step 2: Switch `test` to nextest and add `test-no-run`**

Replace the existing `test` recipe with:

```just
# Run the full test suite (nextest) plus doctests (nextest does not run them).
test:
    cargo nextest run --workspace --all-features
    cargo test --workspace --all-features --doc

# Compile all tests without running them — the fast pre-commit gate.
test-no-run:
    cargo nextest run --workspace --all-features --no-run
```

- [ ] **Step 3: Verify each recipe runs**

Run each and confirm it executes (a tool reporting real findings is a *content* issue to fix, not a recipe failure):

```bash
just fmt-toml || taplo fmt   # if it reports divergence, format then re-run `just fmt-toml`
just typos                   # fix any real typos or add them to _typos.toml extend-words
just machete                 # remove any genuinely unused deps it reports
just gitleaks
just actionlint
just test-no-run
just test
```
Expected: each command exits 0 after any legitimate findings are resolved.

- [ ] **Step 4: Commit**

```bash
git add Justfile _typos.toml Cargo.toml
git commit -m "build: wire taplo/typos/machete/gitleaks/actionlint and nextest into just

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(Include `_typos.toml`/`Cargo.toml` only if Step 3 required fixes there; otherwise commit `Justfile` alone.)

---

### Task 3: Justfile git-helper check recipes

**Files:**
- Modify: `Justfile`

**Interfaces:**
- Consumes: nothing.
- Produces: recipes `check-branch`, `check-merge-conflicts`, `check-large-files`, `commit-msg FILE`. Task 5 hooks and Task 6 aggregates call these.

- [ ] **Step 1: Add the four helper recipes**

Append to `Justfile`:

```just
# Reject commits made directly on the protected 'main' branch.
check-branch:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [[ "$branch" == "main" ]]; then
        echo "✗ Direct commits to 'main' are not allowed; use a feature branch." >&2
        exit 1
    fi

# Reject staged files that contain merge-conflict markers.
check-merge-conflicts:
    #!/usr/bin/env bash
    set -euo pipefail
    files=$(git diff --cached --name-only --diff-filter=ACM)
    [[ -z "$files" ]] && exit 0
    # {7} avoids embedding a literal marker in this recipe.
    if grep -EIln '^(<{7}|={7}|>{7})( |$)' $files; then
        echo "✗ Merge-conflict markers found in staged files." >&2
        exit 1
    fi

# Reject staged files larger than 5 MiB.
check-large-files:
    #!/usr/bin/env bash
    set -euo pipefail
    max=5242880  # 5 MiB
    fail=0
    while IFS= read -r f; do
        size=$(git cat-file -s ":$f" 2>/dev/null || echo 0)
        if (( size > max )); then
            echo "✗ $f is $((size / 1048576)) MiB (limit 5 MiB)." >&2
            fail=1
        fi
    done < <(git diff --cached --name-only --diff-filter=ACM)
    exit $fail

# Validate a commit-message file against Conventional Commits.
commit-msg FILE:
    #!/usr/bin/env bash
    set -euo pipefail
    subject=$(grep -vE '^\s*(#|$)' "{{FILE}}" | head -n1)
    [[ "$subject" =~ ^(Merge|Revert) ]] && exit 0
    pattern='^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9 ,./_-]+\))?!?: .+'
    if [[ ! "$subject" =~ $pattern ]]; then
        echo "✗ Commit message is not a valid Conventional Commit:" >&2
        echo "    $subject" >&2
        echo "  Expected: <type>(<scope>)?: <description>" >&2
        echo "  Types: feat fix docs style refactor perf test build ci chore revert" >&2
        exit 1
    fi
```

- [ ] **Step 2: Verify `commit-msg` rejects a bad message**

Run:
```bash
printf 'broken message\n' > /tmp/msg.txt && just commit-msg /tmp/msg.txt; echo "exit=$?"
```
Expected: prints the rejection text, `exit=1`.

- [ ] **Step 3: Verify `commit-msg` accepts a good message**

Run:
```bash
printf 'feat(engine): add builder\n' > /tmp/msg.txt && just commit-msg /tmp/msg.txt; echo "exit=$?"
```
Expected: no output, `exit=0`.

- [ ] **Step 4: Verify `check-branch` and `check-large-files` run**

Run:
```bash
just check-branch; echo "branch exit=$?"        # on feat/githooks-improvements => exit 0
just check-merge-conflicts; echo "mc exit=$?"   # clean tree => exit 0
just check-large-files; echo "lf exit=$?"       # no large staged files => exit 0
```
Expected: all `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add Justfile
git commit -m "build: add branch/conflict/large-file/commit-msg guard recipes

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Git hooks

**Files:**
- Create: `.githooks/pre-commit`, `.githooks/commit-msg`, `.githooks/pre-push`
- Modify: `Justfile` (add `shellcheck` recipe — defined here so its `.githooks/*` glob has targets)

**Interfaces:**
- Consumes: `just pre-commit`, `just commit-msg FILE`, `just pre-push` (Task 6); `just check-*`/`commit-msg` (Task 3).
- Produces: executable hooks bound via `git config core.hooksPath .githooks` (the existing `setup` recipe); a `shellcheck` recipe used by Task 6's `ci`.

> Tasks 3 and 6 define the recipes these hooks call; create the hooks now but full end-to-end hook firing is verified after Task 6.

- [ ] **Step 1: Create `.githooks/pre-commit`**

```bash
#!/usr/bin/env bash
set -euo pipefail
exec just pre-commit
```

- [ ] **Step 2: Create `.githooks/commit-msg`**

```bash
#!/usr/bin/env bash
set -euo pipefail
exec just commit-msg "$1"
```

- [ ] **Step 3: Create `.githooks/pre-push`**

```bash
#!/usr/bin/env bash
set -euo pipefail
exec just pre-push
```

- [ ] **Step 4: Make the hooks executable**

Run:
```bash
chmod +x .githooks/pre-commit .githooks/commit-msg .githooks/pre-push
```

- [ ] **Step 5: Add the `shellcheck` recipe to the Justfile**

Append to `Justfile`:

```just
# Lint shell scripts: git hooks and devcontainer provisioning.
shellcheck:
    shellcheck .githooks/* .devcontainer/*.sh
```

- [ ] **Step 6: Verify shellcheck passes on the new scripts**

Run: `just shellcheck`
Expected: exit 0 (no warnings). Fix any reported issues in the hook scripts.

- [ ] **Step 7: Verify hooks are bound and fire**

Run:
```bash
just setup
git config --get core.hooksPath          # expect: .githooks
git hook run pre-commit 2>&1 | tail -5    # runs `just pre-commit` once Task 6 lands
```
Expected: `core.hooksPath` is `.githooks`. (If `just pre-commit` is not yet defined, complete Task 6 then re-run.)

- [ ] **Step 8: Commit**

```bash
git add .githooks Justfile
git commit -m "feat: add just-delegating pre-commit/commit-msg/pre-push hooks

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Targeted config fixes

**Files:**
- Modify: `.clippy.toml`, `Cargo.toml`

**Interfaces:**
- Consumes: gap rows from Task 1's audit.
- Produces: `msrv` in clippy config; `keywords`/`categories`/`readme` in workspace metadata.

- [ ] **Step 1: Add `msrv` to `.clippy.toml`**

Add near the top of `.clippy.toml` (after the header comment):

```toml
# Pin Clippy's MSRV so it never suggests APIs newer than our minimum (1.85).
# Must match rust-version in Cargo.toml's [workspace.package].
msrv = "1.85"
```

- [ ] **Step 2: Add metadata to `[workspace.package]` in `Cargo.toml`**

Under `[workspace.package]` (after `description`), add:

```toml
readme = "README.md"
keywords = ["trading", "engine", "finance", "async", "backend-agnostic"]
categories = ["finance", "asynchronous"]
```

- [ ] **Step 3: Verify clippy still passes with the MSRV set**

Run: `just lint`
Expected: exit 0 (no new warnings introduced by the MSRV pin).

- [ ] **Step 4: Verify the manifest is valid**

Run: `cargo metadata --format-version 1 --no-deps > /dev/null && echo OK`
Expected: `OK` (manifest parses; `keywords`/`categories` are valid).

- [ ] **Step 5: Commit**

```bash
git add .clippy.toml Cargo.toml
git commit -m "build: pin clippy msrv and add workspace package metadata

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Justfile aggregates (`pre-commit`, `pre-push`, `ci`)

**Files:**
- Modify: `Justfile`

**Interfaces:**
- Consumes: every recipe from Tasks 2–4 by name.
- Produces: `pre-commit`, `pre-push`, and an updated `ci`. Task 4 hooks and Task 7 CI invoke these.

- [ ] **Step 1: Update the `ci` aggregate**

Replace the existing `ci` recipe with:

```just
# Run the full local CI suite (identical to what .github/workflows/ci.yml invokes).
ci: fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck
```

- [ ] **Step 2: Add the hook aggregates**

Append to `Justfile`:

```just
# Fast pre-commit gate (called by .githooks/pre-commit).
pre-commit: check-branch check-merge-conflicts check-large-files fmt fmt-toml typos lint test-no-run

# Full pre-push gate (called by .githooks/pre-push).
pre-push: ci
```

- [ ] **Step 3: Verify the aggregates resolve and the recipe list is clean**

Run: `just --list`
Expected: lists `ci`, `pre-commit`, `pre-push`, and all atomic recipes with no parse errors.

- [ ] **Step 4: Verify `just ci` passes end to end**

Run: `just ci`
Expected: exit 0. Resolve any real findings surfaced by a tool, then re-run until green.

- [ ] **Step 5: Verify the full pre-commit gate**

Run: `just pre-commit`
Expected: exit 0 on the current clean branch.

- [ ] **Step 6: Commit**

```bash
git add Justfile
git commit -m "build: add pre-commit/pre-push aggregates and expand ci recipe

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: CI workflow — call `just`

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `just ci`, `just msrv`.
- Produces: a CI that mirrors local runs exactly (no duplicated cargo commands).

- [ ] **Step 1: Replace the `ci` job steps with a tool-install + `just ci` flow**

In `.github/workflows/ci.yml`, replace the body of the `ci` job's `steps:` with:

```yaml
    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2

        # Single fast installer for every non-rustc tool the just recipes call.
        # If a tool name errors, check the supported list at
        # https://github.com/taiki-e/install-action and adjust that one name.
      - uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest,cargo-deny,cargo-machete,taplo-cli,typos,gitleaks,actionlint,shellcheck

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
```

Note: this removes the standalone `EmbarkStudios/cargo-deny-action` step — `just ci` now runs `cargo deny` via the installed `cargo-deny`, keeping CI and local identical.

- [ ] **Step 2: Update the `msrv` job to call `just msrv`**

Replace the `msrv` job's `check` step with a just-driven one (keep the separate 1.85 toolchain and its own cache key):

```yaml
      - uses: taiki-e/install-action@v2
        with:
          tool: just

      - name: msrv
        run: just msrv
```
Place these after the existing `Swatinem/rust-cache@v2` (key: msrv) step; remove the old inline `cargo check` step.

- [ ] **Step 3: Lint the workflow locally**

Run: `just actionlint`
Expected: exit 0 (no workflow errors).

- [ ] **Step 4: Sanity-check YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('OK')"`
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: drive all checks through just for local/CI parity

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Push and confirm CI is green**

```bash
git push
gh run watch "$(gh run list --branch feat/githooks-improvements --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```
Expected: both `CI` and `MSRV (1.85)` jobs pass. If a `taiki-e/install-action` tool name failed, fix that single name (Step 1 note) and re-push.

---

## Self-Review

**1. Spec coverage:**
- Phase 0 audit → Task 1 ✓
- Justfile single source of truth → Tasks 2,3,4,6 ✓
- Tool wiring (typos, taplo, machete, nextest, gitleaks, actionlint, shellcheck) → Tasks 2,4 ✓
- Git-helper checks (branch/conflict/large-file 5 MiB/conventional-commit regex) → Task 3 ✓
- Git hooks delegating to just → Task 4 ✓; `setup` recipe revived → Task 4 Step 7 ✓
- CI one job calling just; `msrv` separate job exception → Task 7 ✓
- Targeted fixes (clippy msrv, Cargo metadata) → Task 5 ✓
- Out-of-scope items (nightly rustfmt, toolchain pin, new lint groups, external commit linter) → not introduced ✓

**2. Placeholder scan:** No TBD/TODO left. The `<url>`/table-cell tokens in Task 1's skeleton are an explicitly-required deliverable to fill, with a verification step that fails if left unfilled. The single `taiki-e/install-action` tool-name caveat is bounded guidance with a fix path, not an open placeholder.

**3. Type/name consistency:** Recipe names are consistent across tasks — `fmt-toml`, `typos`, `machete`, `gitleaks`, `actionlint`, `shellcheck`, `test`, `test-no-run`, `check-branch`, `check-merge-conflicts`, `check-large-files`, `commit-msg`, `pre-commit`, `pre-push`, `ci`, `msrv`, `setup`. Hooks call exactly the aggregate names defined in Task 6. CI calls `just ci`/`just msrv` defined in Tasks 6 and the existing Justfile. 5 MiB == 5242880 used consistently.
