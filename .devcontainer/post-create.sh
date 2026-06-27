#!/usr/bin/env bash
# Provision the OATH devcontainer: system libraries, security/lint tooling,
# Rust tooling, and git hooks. Invoked by devcontainer.json postCreateCommand,
# which runs from the workspace root.
#
# Tools are built from source via `cargo install --locked` (not cargo-binstall)
# for build-from-source provenance. There is no cargo cache volume, so this runs
# in full on each container create.
set -euo pipefail

# --- Volume-backed build dir ownership ----------------------------------
# target/ is a Docker named volume (see devcontainer.json `mounts`) for
# native ext4 build I/O instead of the slow 9p bind mount. A fresh volume
# mounts as root, so hand it to the non-root `vscode` user before any build
# writes to it. Idempotent: a no-op once the volume is already owned.
sudo mkdir -p /workspaces/oath/target
sudo chown vscode:vscode /workspaces/oath/target

# --- System packages (Debian trixie) ------------------------------------
# libclang-dev: bindgen/libclang for iceoryx2 build scripts.
# gitleaks, just, shellcheck: security scan, task runner, shell-hook linter.
sudo apt-get update
sudo apt-get install -y \
    libclang-dev \
    gitleaks \
    just \
    shellcheck
sudo rm -rf /var/lib/apt/lists/*

# --- actionlint (Go tool; official binary installer) --------------------
# Downloads the latest release binary into /usr/local/bin.
curl -sSfL https://raw.githubusercontent.com/rhysd/actionlint/main/scripts/download-actionlint.bash \
    | sudo bash -s -- latest /usr/local/bin

# --- Rust tooling (built from source for provenance) --------------------
cargo install --locked \
    cargo-deny \
    cargo-nextest \
    cargo-mutants \
    cargo-machete \
    typos-cli \
    taplo-cli \
    just-lsp

# --- Git hooks ----------------------------------------------------------
# Sets core.hooksPath to .githooks (idempotent).
just setup
