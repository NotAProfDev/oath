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

# Type-check every crate and target without codegen.
check:
    cargo check --workspace --all-targets --all-features

# Run Clippy across every crate and target; warnings are errors.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run the full test suite.
test:
    cargo test --workspace --all-features

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
