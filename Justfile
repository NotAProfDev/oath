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

# Type-check every crate and target without codegen.
check:
    cargo check --workspace --all-targets

# Run Clippy across every crate and target; warnings are errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the full test suite.
test:
    cargo test --workspace

# Check licenses, bans, advisories, and sources.
deny:
    cargo deny --all-features check

# Run the full local CI suite: lint, test, and deny.
ci: lint test deny
