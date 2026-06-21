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
