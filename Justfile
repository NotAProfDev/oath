# Just task runner for OATH.
# Full reference: https://just.systems/man/en/
#
# Install: https://just.systems/man/en/packages.html
# Usage:   just <recipe>  (e.g. `just ci`)

# List all available recipes (default when running `just` with no arguments).
[private]
default:
    @just --list

# Point Git at the project's hook directory. Run once after cloning outside the devcontainer.
setup:
    git config core.hooksPath .githooks

# Reject any formatting divergence. Fix with: cargo fmt --all
fmt:
    cargo fmt --all -- --check

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

# Type-check every crate and target without codegen.
check:
    cargo check --workspace --all-targets --all-features

# Run Clippy across every crate and target; warnings are errors.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run the full test suite (nextest) plus doctests (nextest does not run them).
test:
    cargo nextest run --workspace --all-features --no-tests=pass
    cargo test --workspace --all-features --doc

# Compile all tests without running them — the fast pre-commit gate.
test-no-run:
    cargo nextest run --workspace --all-features --no-run

# Check licenses, bans, advisories, and sources.
deny:
    cargo deny --all-features check

# Build docs; broken intra-doc links and rustdoc warnings are hard errors.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --all-features

# Verify compilation on the declared MSRV. Requires: rustup toolchain install 1.85
msrv:
    cargo +1.85 check --workspace --all-targets --all-features

# Run the full local CI suite (matches .github/workflows/ci.yml).
ci: fmt lint test deny doc

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
    mapfile -d '' -t files < <(git diff --cached --name-only --diff-filter=ACM -z)
    [[ ${#files[@]} -eq 0 ]] && exit 0
    # {7} avoids embedding a literal marker in this recipe.
    if grep -EIln '^(<{7}|={7}|>{7})( |$)' "${files[@]}"; then
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

# Lint shell scripts: git hooks and devcontainer provisioning.
shellcheck:
    shellcheck .githooks/* .devcontainer/*.sh

# Validate a commit-message file against Conventional Commits.
commit-msg FILE:
    #!/usr/bin/env bash
    set -euo pipefail
    subject=$(grep -vE '^\s*(#|$)' "{{FILE}}" | head -n1 || true)
    [[ "$subject" =~ ^(Merge|Revert) ]] && exit 0
    pattern='^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9 ,./_-]+\))?!?: .+'
    if [[ ! "$subject" =~ $pattern ]]; then
        echo "✗ Commit message is not a valid Conventional Commit:" >&2
        echo "    $subject" >&2
        echo "  Expected: <type>(<scope>)?: <description>" >&2
        echo "  Types: feat fix docs style refactor perf test build ci chore revert" >&2
        exit 1
    fi
