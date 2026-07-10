# Just task runner for OATH.
# Full reference: https://just.systems/man/en/
#
# Install: https://just.systems/man/en/packages.html
# Usage:   just <recipe>  (e.g. `just ci`)

# ── Configuration ─────────────────────────────────────────────────────────────

# Shared scope for the compile/lint recipes.
SCOPE := "--workspace --all-targets --all-features"

# Largest blob (bytes) the pre-commit hook will let into a commit (5 MiB).
MAX_FILE_SIZE := "5242880"

# Branches that commits/pushes must never land on directly — work happens on
# feature branches via PRs. Space-separated; checked by check-branch/check-push.
PROTECTED_BRANCHES := "main"

# ── Setup ─────────────────────────────────────────────────────────────────────

# List all available recipes (default when running `just` with no arguments).
[private]
default:
    @just --list

# Point Git at the project's hook directory and ensure the hooks are executable.
# The chmod is a self-heal: hooks are committed with the exec bit, but a checkout
# that strips it (archive export, permission-flattening fs) leaves git silently
# skipping them — git only runs hooks it can execute.
[doc('Wire git to .githooks and make the hooks executable (run once per non-devcontainer clone).')]
setup:
    git config core.hooksPath .githooks
    chmod +x .githooks/*

# ── Format & lint ─────────────────────────────────────────────────────────────

# Reject any formatting divergence. Fix with: just fix (or cargo fmt --all)
fmt:
    cargo fmt --all -- --check

# Reject any TOML formatting divergence. Fix with: taplo fmt
fmt-toml:
    taplo fmt --check

# Auto-fix what tooling can: format the workspace and apply Clippy's machine-applicable fixes.
fix:
    cargo fmt --all
    cargo clippy {{SCOPE}} --fix --allow-dirty --allow-staged

# Type-check every crate and target without codegen.
check:
    cargo check {{SCOPE}} --locked

# Run Clippy across every crate and target; warnings are errors.
lint:
    cargo clippy {{SCOPE}} --locked -- -D warnings

# Verify compilation on the declared MSRV. Requires: rustup toolchain install 1.90
msrv:
    cargo +1.90 check --workspace --all-targets --all-features

# ── Test ──────────────────────────────────────────────────────────────────────

# Run the full test suite (nextest) plus doctests (nextest does not run them).
test:
    cargo nextest run --workspace --all-features --locked --no-tests=pass
    cargo test --workspace --all-features --locked --doc

# Compile all tests without running them — the fast pre-commit gate.
test-no-run:
    cargo nextest run --workspace --all-features --locked --no-run

# ── Static analysis ───────────────────────────────────────────────────────────

# Spell-check the repository (config: _typos.toml).
typos:
    typos

# Scan the worktree and the current branch's history for committed secrets.
# Scope to HEAD's ancestry (not every local ref): the gate is about what this
# branch would push, and it keeps a multi-branch local clone from tripping on
# unrelated branches' history — matching what CI scans on a single-branch checkout.
gitleaks:
    gitleaks detect --no-banner --log-opts="HEAD"

# Lint GitHub Actions workflow files.
actionlint:
    actionlint

# Lint shell scripts: git hooks, devcontainer provisioning, and the IBKR capture harness.
shellcheck:
    shellcheck .githooks/* .devcontainer/*.sh docker/cpapi/*.sh

# ── IBKR fixture capture ──────────────────────────────────────────────────────

# Capture Client Portal API v1 read-path fixtures from a running, authenticated
# gateway (see docker/cpapi/README.md). Pass a paper account id. The gateway base
# URL is resolved to the container's bridge IP because localhost:5000 is not
# routable from inside a devcontainer.
ibkr-capture account="":
    IBKR_GATEWAY="$(docker/cpapi/gateway-base-url.sh)" docker/cpapi/capture.sh {{account}}

# ── Supply chain & docs ───────────────────────────────────────────────────────

# Check licenses, bans, advisories, and sources.
deny:
    cargo deny --all-features check

# Detect dependencies declared in Cargo.toml but never used.
machete:
    cargo machete

# Build docs; broken intra-doc links and rustdoc warnings are hard errors.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --all-features

# ── Mutation testing ──────────────────────────────────────────────────────────

# Mutation testing — verify the suite actually catches bugs. Slow (minutes+).
mutants:
    cargo mutants --workspace

# Fast mutation testing on lines changed vs origin/main — for local/PR loops.
mutants-diff:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target
    # Diff the merge-base with origin/main against the WORKING TREE, not HEAD:
    # cargo mutants --in-diff matches the diff's "after" side to the source it
    # mutates (the working tree), so a committed-only origin/main...HEAD diff
    # mismatches the moment you have uncommitted edits — which is the point here.
    base=$(git merge-base origin/main HEAD)
    git diff "$base" > target/mutants.diff
    cargo mutants --in-diff target/mutants.diff

# ── Aggregate gates ───────────────────────────────────────────────────────────

# Run the full local CI suite (identical to what .github/workflows/ci.yml invokes).
ci: fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck

# Fast pre-commit gate (called by .githooks/pre-commit).
pre-commit: check-branch check-merge-conflicts check-large-files fmt fmt-toml typos lint test-no-run

# Full pre-push gate (called by .githooks/pre-push). check-push first so it
# consumes git's piped ref list before the CI steps run.
[doc('Full pre-push gate: protected-target check, then the full CI gate.')]
pre-push: check-push ci

# ── Git-hook internals ────────────────────────────────────────────────────────
# Building blocks of the pre-commit/pre-push gates above. [private] keeps them out
# of `just --list`; they are invoked by the aggregate gates, not run by hand
# (check-push in particular blocks on stdin if run without git's piped ref list).

# Reject commits made directly on a protected branch (see PROTECTED_BRANCHES).
# A detached HEAD yields an empty name and passes.
[private]
check-branch:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git symbolic-ref --short -q HEAD || echo "")
    for protected in {{PROTECTED_BRANCHES}}; do
        if [[ "$branch" == "$protected" ]]; then
            echo "✗ Direct commits to '$branch' are not allowed; use a feature branch." >&2
            exit 1
        fi
    done

# Reject any push whose TARGET is a protected branch. Reads git's pre-push stdin
# protocol ("<local ref> <local oid> <remote ref> <remote oid>" per line), so it
# catches pushing to a protected branch from ANY local branch (e.g.
# git push origin HEAD:main), which the check-branch guard would miss.
# Hook-internal: it blocks on read if invoked without git's piped stdin.
[private]
check-push:
    #!/usr/bin/env bash
    set -euo pipefail
    blocked=0
    while read -r _local_ref _local_oid remote_ref _remote_oid; do
        [[ -z "${remote_ref:-}" ]] && continue
        for protected in {{PROTECTED_BRANCHES}}; do
            if [[ "$remote_ref" == "refs/heads/$protected" ]]; then
                echo "✗ Push targets protected branch '$protected' — open a PR instead." >&2
                blocked=1
            fi
        done
    done
    exit "$blocked"

# Reject staged content that still contains merge-conflict markers.
[private]
check-merge-conflicts:
    #!/usr/bin/env bash
    set -euo pipefail
    found=0
    while IFS= read -r -d '' f; do
        # Inspect the STAGED blob (git show ":$f"), not the working tree, so the
        # gate matches what will actually be committed. Key on the angle-bracket
        # markers only: a 7-char run of `=` is common in legit content (Markdown
        # setext headings, ASCII rules), and a real conflict always carries the
        # angle brackets — so keying on those alone stays complete without the
        # false positives.
        if git show ":$f" 2>/dev/null | grep -nE '^(<{7}|>{7})( |$)' >/dev/null; then
            echo "✗ Merge-conflict markers in $f" >&2
            found=1
        fi
    done < <(git diff --cached --name-only --diff-filter=ACM -z)
    if [[ "$found" -ne 0 ]]; then
        echo "Resolve the conflicts before committing (or git commit --no-verify to override)." >&2
        exit 1
    fi

# Reject staged blobs larger than MAX_FILE_SIZE (default 5 MiB).
[private]
check-large-files limit=MAX_FILE_SIZE:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0
    while IFS= read -r -d '' f; do
        size=$(git cat-file -s ":$f" 2>/dev/null || echo 0)
        if (( size > {{limit}} )); then
            echo "✗ $f is $((size / 1048576)) MiB (limit $(( {{limit}} / 1048576 )) MiB)." >&2
            fail=1
        fi
    done < <(git diff --cached --name-only --diff-filter=ACM -z)
    exit $fail

# Validate a commit-message file against Conventional Commits.
commit-msg FILE:
    #!/usr/bin/env bash
    set -euo pipefail
    # Count the subject in characters, not bytes: bash's ${#subject} counts bytes
    # under LC_ALL=C, so a ≤72-char subject using multi-byte chars (em-dash) would
    # be wrongly rejected. C.UTF-8 ships in the base image.
    export LC_ALL=C.UTF-8
    # Subject = first line that is neither blank nor a git comment.
    subject=$(grep -vE '^\s*(#|$)' "{{FILE}}" | head -n1 || true)
    # Let git's own machinery through: empty (aborts), merges, reverts, fixup/squash/amend.
    case "$subject" in
        "" | Merge\ * | Revert\ * | fixup!\ * | squash!\ * | amend!\ * ) exit 0 ;;
    esac
    pattern='^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9 ,./_-]+\))?!?: .+'
    if [[ ! "$subject" =~ $pattern ]]; then
        echo "✗ Commit message is not a valid Conventional Commit:" >&2
        echo "    $subject" >&2
        echo "  Expected: <type>(<scope>)?: <description>" >&2
        echo "  Types: feat fix docs style refactor perf test build ci chore revert" >&2
        exit 1
    fi
    if (( ${#subject} > 72 )); then
        echo "✗ Commit subject is ${#subject} chars (max 72) — shorten it." >&2
        exit 1
    fi

# ── Self-tests ────────────────────────────────────────────────────────────────

# Self-test the commit-msg rules: feed known-good/bad subjects through the recipe
# and assert the exit codes. Run with: just test-commit-msg
[doc('Self-test the commit-msg rules (asserts exit codes for good/bad subjects).')]
test-commit-msg:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT
    pass=0 fail=0
    expect() { # <expected-exit> <message>
        printf '%s\n' "$2" > "$tmp"
        if just commit-msg "$tmp" >/dev/null 2>&1; then got=0; else got=$?; fi
        if [[ "$got" -eq "$1" ]]; then
            pass=$((pass + 1))
        else
            fail=$((fail + 1)); echo "FAIL: expected exit $1, got $got for: $2" >&2
        fi
    }
    # Valid Conventional Commits.
    expect 0 "feat: add ingestion pipeline"
    expect 0 "fix(api): reject empty tenant id"
    expect 0 "feat(detection)!: drop legacy rule format"
    expect 0 "chore: bump toolchain to 1.95 (#7)"
    expect 0 "Merge branch 'main' into feat/x"
    expect 0 "Revert \"feat: oops\""
    expect 0 "fixup! feat: add ingestion pipeline"
    # Invalid.
    expect 1 "add ingestion pipeline"
    expect 1 "Feat: capitalized type"
    expect 1 "feat add ingestion pipeline"
    expect 1 "wip: not an allowed type"
    expect 1 "feat: $(printf 'x%.0s' {1..80})"
    echo "commit-msg self-test: $pass passed, $fail failed"
    [[ "$fail" -eq 0 ]]
