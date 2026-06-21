# Devcontainer Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve the OATH devcontainer — add the `gh` CLI plus extra Rust/lint/security tooling and editor extensions, and make provisioning maintainable — per the approved spec at `docs/superpowers/specs/2026-06-21-devcontainer-improvements-design.md`.

**Architecture:** Tooling is installed in three tiers: devcontainer Features (declarative; `github-cli` + existing `docker-outside-of-docker`), apt packages, and `cargo install --locked` Rust binaries. The previously inline `postCreateCommand` is extracted into a maintainable, shellcheck-able `.devcontainer/post-create.sh` script that runs the apt installs, `actionlint` install, cargo installs, and `just setup`. A repo-root `_typos.toml` gives the `typos` CLI and the `tekumara.typos-vscode` extension one shared config. `gh` authenticates via VS Code Dev Containers host-credential forwarding.

**Tech Stack:** Dev Containers (devcontainer.json + Features), Debian trixie apt, Rust/cargo, `gh`, shellcheck, gitleaks, just, typos, taplo, actionlint, cargo-{deny,nextest,mutants,machete}.

## Global Constraints

- **Installation tiers:** Features for `github-cli`; apt for `libclang-dev`, `gitleaks`, `just`, `shellcheck`; `cargo install --locked` for `cargo-deny`, `cargo-nextest`, `cargo-mutants`, `cargo-machete`, `typos-cli`, `taplo-cli`, `just-lsp`; official binary installer for `actionlint`.
- **Features float to latest** — no version pinning beyond the existing major (`:1`).
- **No persistent mounts / no cargo cache volume / no sccache.**
- **Rust tooling built from source** via `cargo install --locked` (not `cargo-binstall`).
- **No secrets in the repo** — `gh` auth relies on host-credential forwarding; do not add tokens to `containerEnv`/`remoteEnv`.
- **Out of scope:** MSRV toolchain in-container; wiring shellcheck/typos into git hooks; version-pinning features.
- **Commit messages:** Conventional Commits (enforced by `.githooks/commit-msg`): `feat:`, `chore:`, `docs:`, etc.
- **`postCreateCommand` runs from the workspace root** (`/workspaces/oath`), so relative paths like `.devcontainer/post-create.sh` and `.githooks` resolve correctly.

---

### Task 1: Extract provisioning into `.devcontainer/post-create.sh`

Move container provisioning out of the inline `postCreateCommand` string into a dedicated script, and include the full final tool set (apt packages, actionlint, cargo tools, git-hook wiring). This is the core provisioning deliverable; the Feature and extension edits in later tasks are independent.

**Files:**
- Create: `.devcontainer/post-create.sh`
- Modify: `.devcontainer/devcontainer.json` (the `postCreateCommand` line, currently line 24)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a script at `.devcontainer/post-create.sh` invoked by `postCreateCommand`. Installs onto `PATH`: `gitleaks`, `just`, `shellcheck`, `actionlint`, `cargo-deny`, `cargo-nextest`, `cargo-mutants`, `cargo-machete`, `typos`, `taplo`, `just-lsp`, plus system lib `libclang-dev`. Runs `just setup` to set `core.hooksPath`.

- [ ] **Step 1: Create the provisioning script**

Create `.devcontainer/post-create.sh`:

```bash
#!/usr/bin/env bash
# Provision the OATH devcontainer: system libraries, security/lint tooling,
# Rust tooling, and git hooks. Invoked by devcontainer.json postCreateCommand,
# which runs from the workspace root.
#
# Tools are built from source via `cargo install --locked` (not cargo-binstall)
# for build-from-source provenance. There is no cargo cache volume, so this runs
# in full on each container create.
set -euo pipefail

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
```

- [ ] **Step 2: Make the script executable**

Run: `chmod +x .devcontainer/post-create.sh`

- [ ] **Step 3: Verify the script has valid shell syntax**

Run: `bash -n .devcontainer/post-create.sh`
Expected: no output, exit code 0.

- [ ] **Step 4: Point `postCreateCommand` at the script**

In `.devcontainer/devcontainer.json`, replace the existing `postCreateCommand` line:

```json
	"postCreateCommand": "sudo apt-get update && sudo apt-get install -y libclang-dev gitleaks just && sudo rm -rf /var/lib/apt/lists/* && cargo install --locked cargo-deny && just setup",
```

with:

```json
	// Provision the container (tools + git hooks). See .devcontainer/post-create.sh.
	"postCreateCommand": "bash .devcontainer/post-create.sh",
```

- [ ] **Step 5: Commit**

```bash
git add .devcontainer/post-create.sh .devcontainer/devcontainer.json
git commit -m "refactor: extract devcontainer provisioning into post-create.sh

Move apt + cargo installs into a maintainable, shellcheck-able script and
add the new tooling (shellcheck, actionlint, cargo-nextest/mutants/machete,
typos, taplo, just-lsp)."
```

---

### Task 2: Add the `github-cli` Feature

Add `gh` via the official devcontainer Feature, alongside the existing docker Feature.

**Files:**
- Modify: `.devcontainer/devcontainer.json` (the `features` block, currently lines 17-21)

**Interfaces:**
- Consumes: nothing.
- Produces: `gh` on `PATH` inside the container; VS Code Dev Containers forwards host `gh` credentials automatically when this Feature is present.

- [ ] **Step 1: Add the Feature to the `features` block**

In `.devcontainer/devcontainer.json`, change the `features` block from:

```json
	"features": {
		"ghcr.io/devcontainers/features/docker-outside-of-docker:1": {
			"moby": false
		}
	},
```

to:

```json
	"features": {
		"ghcr.io/devcontainers/features/docker-outside-of-docker:1": {
			"moby": false
		},
		// GitHub CLI. VS Code forwards host `gh` credentials into the
		// container automatically; run `gh auth login` if not authed on host.
		"ghcr.io/devcontainers/features/github-cli:1": {}
	},
```

- [ ] **Step 2: Verify the edited region is still well-formed JSONC**

Run: `grep -n "github-cli" .devcontainer/devcontainer.json`
Expected: one match showing the new Feature line. (Full JSON validity is verified by the rebuild in Task 5; devcontainer.json is JSONC and contains `//` comments, so standard JSON parsers will reject it — do not lint it with `jq`/`json.tool`.)

- [ ] **Step 3: Commit**

```bash
git add .devcontainer/devcontainer.json
git commit -m "feat: add github-cli feature to devcontainer"
```

---

### Task 3: Update VSCode extensions

Add the agreed extensions. Note: `streetsidesoftware.code-spell-checker` is intentionally NOT added — `tekumara.typos-vscode` replaces it.

**Files:**
- Modify: `.devcontainer/devcontainer.json` (the `extensions` array, currently starting line 28)

**Interfaces:**
- Consumes: nothing.
- Produces: `tekumara.typos-vscode` installed, which reads the `_typos.toml` created in Task 4.

- [ ] **Step 1: Replace the `extensions` array**

In `.devcontainer/devcontainer.json`, change the `extensions` array from:

```json
				"extensions": [
					"rust-lang.rust-analyzer",
					"fill-labs.dependi",
					"tamasfe.even-better-toml",
					"github.vscode-github-actions",
					"anthropic.claude-code"
				],
```

to:

```json
				"extensions": [
					// Rust
					"rust-lang.rust-analyzer",
					"vadimcn.vscode-lldb",
					"fill-labs.dependi",
					// Config / formats
					"tamasfe.even-better-toml",
					"editorconfig.editorconfig",
					"mikestead.dotenv",
					"davidanson.vscode-markdownlint",
					// Spell-check (same `typos` engine/config as CLI + CI)
					"tekumara.typos-vscode",
					// Diagnostics
					"usernamehw.errorlens",
					// Git / GitHub
					"eamodio.gitlens",
					"github.vscode-github-actions",
					"github.vscode-pull-request-github",
					// Docker (docker-outside-of-docker feature)
					"ms-azuretools.vscode-docker",
					// Claude Code
					"anthropic.claude-code"
				],
```

- [ ] **Step 2: Verify the new extension IDs are present**

Run: `grep -cE 'vscode-lldb|errorlens|vscode-pull-request-github|editorconfig|vscode-docker|gitlens|markdownlint|mikestead.dotenv|typos-vscode' .devcontainer/devcontainer.json`
Expected: `9`

- [ ] **Step 3: Confirm code-spell-checker was NOT added**

Run: `! grep -q "code-spell-checker" .devcontainer/devcontainer.json && echo "ok: not present"`
Expected: `ok: not present`

- [ ] **Step 4: Commit**

```bash
git add .devcontainer/devcontainer.json
git commit -m "feat: add editor extensions for debugging, github, docs, typos"
```

---

### Task 4: Add `_typos.toml`

Add a repo-root typos config shared by the CLI (future hooks/CI) and the `typos-vscode` extension.

**Files:**
- Create: `_typos.toml`

**Interfaces:**
- Consumes: read by `tekumara.typos-vscode` (Task 3) and by the `typos` CLI (Task 1).
- Produces: project allowlist; future hook/CI integration (out of scope) will run `typos` against it.

- [ ] **Step 1: Create the config**

Create `_typos.toml`:

```toml
# Shared typos configuration for the OATH repo.
# Consumed by the `typos` CLI (run manually, and by future git-hook/CI
# integration) and by the tekumara.typos-vscode editor extension, so
# spell-checking is identical in the editor and on the command line.
# Reference: https://github.com/crate-ci/typos/blob/master/docs/reference.md

[files]
# Generated or vendored files we never spell-check.
extend-exclude = ["Cargo.lock", "target/"]

[default.extend-words]
# Project-specific terms / acronyms that are valid (map each word to itself).
# Add an entry here whenever `typos` reports a false positive.
oath = "oath"
```

- [ ] **Step 2: Verify it is valid TOML**

Run: `python3 -c "import tomllib,sys; tomllib.load(open('_typos.toml','rb')); print('ok')"`
Expected: `ok`

(After a container rebuild, `typos` itself validates the config: `typos --files` runs clean.)

- [ ] **Step 3: Commit**

```bash
git add _typos.toml
git commit -m "chore: add shared _typos.toml spell-check config"
```

---

### Task 5: Document `gh` auth and verify the rebuilt container

Add the user-facing note about `gh` authentication, then rebuild and run the full validation checklist from the spec.

**Files:**
- Modify: `README.md` (the `## Setup` section)

**Interfaces:**
- Consumes: everything from Tasks 1-4 (the fully provisioned container).
- Produces: documented auth flow; verified working environment.

- [ ] **Step 1: Document gh auth in the README**

In `README.md`, replace the `## Setup` section:

```markdown
## Setup

After cloning, activate the local git hooks:

```sh
git config core.hooksPath .githooks
```

This is done automatically inside the dev container.
```

with:

```markdown
## Setup

After cloning, activate the local git hooks:

```sh
git config core.hooksPath .githooks
```

This is done automatically inside the dev container.

### Dev container

The dev container provisions all tooling (`gh`, `just`, `gitleaks`,
`shellcheck`, `actionlint`, `typos`, `taplo`, and the `cargo-*` tools) via
[`.devcontainer/post-create.sh`](.devcontainer/post-create.sh).

The GitHub CLI authenticates by forwarding your host credentials. If you are
already signed in with `gh` on your machine, `gh` works inside the container
with no extra steps. Otherwise, run `gh auth login` once inside the container.
```

- [ ] **Step 2: Commit the docs**

```bash
git add README.md
git commit -m "docs: document devcontainer tooling and gh auth"
```

- [ ] **Step 3: Rebuild the container**

In VS Code: Command Palette → **Dev Containers: Rebuild Container**. Wait for `post-create.sh` to finish (it builds Rust tools from source, so this takes several minutes).

- [ ] **Step 4: Verify every tool resolves on PATH**

Run inside the rebuilt container:

```sh
for t in gh shellcheck actionlint typos taplo just-lsp; do command -v "$t" >/dev/null && echo "ok: $t" || echo "MISSING: $t"; done
for c in deny nextest mutants machete; do cargo "$c" --version >/dev/null 2>&1 && echo "ok: cargo-$c" || echo "MISSING: cargo-$c"; done
```

Expected: every line starts with `ok:`.

- [ ] **Step 5: Verify CI parity**

Run: `just ci`
Expected: `fmt`, `lint`, `test`, `deny`, `doc` all pass (matches `.github/workflows/ci.yml`).

- [ ] **Step 6: Verify gh auth**

Run: `gh auth status`
Expected: reports an authenticated github.com account (when the host is authed). If not, `gh auth login` is the documented fallback.

- [ ] **Step 7: Verify typos config is honored**

Run: `typos`
Expected: completes; no errors about an invalid `_typos.toml`.

---

## Self-Review

**Spec coverage:**
- Three install tiers (Features / apt / `cargo install --locked`) → Task 1 (apt + cargo) + Task 2 (Feature). ✓
- `github-cli` Feature, float to latest → Task 2. ✓
- shellcheck, gitleaks, just via apt → Task 1. ✓
- cargo-deny/nextest/mutants/machete, typos-cli, taplo-cli, just-lsp via `cargo install --locked` → Task 1. ✓
- actionlint via official installer → Task 1. ✓
- No cargo cache mount / no sccache / no binstall → enforced by Global Constraints; Task 1 builds from source, no mounts added. ✓
- Extensions added + `code-spell-checker` not added → Task 3 (Step 3 asserts absence). ✓
- `_typos.toml` starter → Task 4. ✓
- `gh` host-credential forwarding + `gh auth login` fallback, no secrets → Task 2 (Feature comment) + Task 5 (README, `gh auth status`). ✓
- Validation checklist (tools on PATH, `just ci`, `gh auth status`) → Task 5 Steps 4-7. ✓
- MSRV / hook-wiring / pinning left out → Global Constraints "Out of scope." ✓

**Placeholder scan:** No TBD/TODO; every code/config block is complete. ✓

**Type consistency:** Tool/crate names (`typos-cli`→`typos`, `taplo-cli`→`taplo`, `just-lsp`, `cargo-*`) and extension IDs are consistent between Task 1, Task 3, Task 4, and the Task 5 verification loop. ✓
