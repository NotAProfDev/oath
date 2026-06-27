# Devcontainer Volume Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the write-heavy Rust build directories (`target/` and `CARGO_HOME`) off the slow 9p `D:\` bind mount onto native Docker named volumes, keeping source on the Windows share.

**Architecture:** The workspace is bind-mounted from `D:\` over the 9p protocol (`msize=65536`), which is fine for editing source but punishing for the thousands of small-file writes Rust compilation produces — `target/` alone is already 5.8 GB. We keep the source on the bind mount (so it stays visible/editable from Windows) and redirect only `/workspaces/oath/target` and `/usr/local/cargo` to Docker **named volumes**, which live in the WSL2 ext4 filesystem at near-native speed. A fresh volume mounts as `root`, so `post-create.sh` hands `target/` to the non-root `vscode` user before any build writes to it.

**Tech Stack:** Dev Containers spec (`devcontainer.json` `mounts`), Docker named volumes, bash (`post-create.sh`), Cargo, `just`.

## Global Constraints

- **Conventional Commits**, enforced by the `commit-msg` hook — use `chore(devcontainer): …` for these changes.
- **Don't bypass git hooks.** Work is done only when `just ci` passes (fmt, lint, test, doc, deny, typos, shellcheck on hooks/scripts).
- **`devcontainer.json` is JSONC** — comments are allowed; the file must stay valid (trailing-comma rules apply per object/array).
- **`post-create.sh` must pass `shellcheck`** (it is `set -euo pipefail`) — keep that header intact.
- **Workspace folder is `/workspaces/oath`** and the container runs as user `vscode` (uid/gid 1000).
- **Repo workflow:** one issue → one branch off `main` (`chore/devcontainer-volume-cache`) → PR referencing the issue → squash-merge.

---

### Task 1: Add named volumes for `target/` and `CARGO_HOME` to `devcontainer.json`

Replace the commented-out `mounts` placeholder with an active `mounts` array declaring two named volumes. `${devcontainerId}` scopes each volume to this devcontainer config so it survives rebuilds but doesn't collide with other projects. Docker auto-populates a fresh named volume from the image's contents at that path on first create — so `/usr/local/cargo` (which exists in the image with the toolchain + `vscode`/`rustlang` permissions) is preserved, while `/workspaces/oath/target` (absent from the image) comes up empty and root-owned, to be fixed in Task 2.

**Files:**
- Modify: `.devcontainer/devcontainer.json:11-18` (the commented `// "mounts": [ ... ]` block)

**Interfaces:**
- Consumes: nothing (entry point).
- Produces: two Docker named volumes mounted at `/workspaces/oath/target` and `/usr/local/cargo`. Task 2 relies on the mount path `/workspaces/oath/target` existing and being root-owned at first create.

- [ ] **Step 1: Replace the commented `mounts` placeholder with an active `mounts` array**

In [.devcontainer/devcontainer.json](.devcontainer/devcontainer.json#L11-L18), replace exactly this block:

```jsonc
  // Use 'mounts' to make the cargo cache persistent in a Docker Volume.
  // "mounts": [
  // 	{
  // 		"source": "devcontainer-cargo-cache-${devcontainerId}",
  // 		"target": "/usr/local/cargo",
  // 		"type": "volume"
  // 	}
  // ]
```

with this:

```jsonc
  // Keep source on the (9p) Windows bind mount, but redirect Rust's
  // write-heavy dirs to native Docker named volumes for fast build I/O.
  // Volumes are scoped per devcontainer config via ${devcontainerId} and
  // survive rebuilds. See docs/superpowers/plans/2026-06-27-devcontainer-volume-cache.md
  "mounts": [
    // Compiler output: thousands of small writes — the main 9p bottleneck.
    // Absent from the image, so this volume starts empty + root-owned;
    // post-create.sh chowns it to `vscode`.
    {
      "source": "oath-target-${devcontainerId}",
      "target": "/workspaces/oath/target",
      "type": "volume"
    },
    // Crate registry + tools installed by post-create.sh. Docker populates
    // this volume from the image's /usr/local/cargo (perms preserved), so it
    // also persists `cargo install` output across rebuilds.
    {
      "source": "oath-cargo-cache-${devcontainerId}",
      "target": "/usr/local/cargo",
      "type": "volume"
    }
  ],
```

- [ ] **Step 2: Verify the file is still valid JSONC**

Authoritative check: open `.devcontainer/devcontainer.json` in the editor — its
built-in JSONC language service flags stray/missing commas and brackets inline.
(The container rebuild in Task 3 is the ultimate validator; the devcontainer CLI
rejects malformed config.)

Optional quick smoke test (bracket/comma sanity only): strip line comments and
parse —

`python3 -c "import json,re; s=open('.devcontainer/devcontainer.json').read(); s=re.sub(r'(^|\s)//.*$','',s,flags=re.M); json.loads(s); print('parses')"`

Expected: prints `parses`. Caveat: this regex is *not* JSONC-aware — it can
misparse a `//` that appears inside a string value, so a failure here is only
meaningful after confirming no string contains ` //`. Trust the editor/rebuild
over this check.

- [ ] **Step 3: Commit**

```bash
git add .devcontainer/devcontainer.json
git commit -m "chore(devcontainer): mount target/ and cargo cache as named volumes"
```

---

### Task 2: Give the `vscode` user ownership of the fresh `target` volume

The `target` named volume from Task 1 mounts as `root:root` on first create because the path doesn't exist in the image to populate it. The container runs as `vscode` (uid 1000), so without a chown the first `cargo build`/`rust-analyzer` write fails with `Permission denied`. `post-create.sh` runs after mounts are established, so it is the right place to fix ownership. `CARGO_HOME` needs no chown — its volume is populated from the image, preserving the existing `vscode:rustlang` group-writable permissions.

**Files:**
- Modify: `.devcontainer/post-create.sh:9` (insert a new section immediately after `set -euo pipefail`)

**Interfaces:**
- Consumes: the `/workspaces/oath/target` mount created in Task 1.
- Produces: a `target/` directory writable by `vscode`, ready for `cargo`/`just` builds.

- [ ] **Step 1: Insert the ownership-fix section after the `set -euo pipefail` line**

In [.devcontainer/post-create.sh](.devcontainer/post-create.sh#L9), find this line:

```bash
set -euo pipefail
```

and insert immediately after it (preserving the blank line that follows):

```bash

# --- Volume-backed build dir ownership ----------------------------------
# target/ is a Docker named volume (see devcontainer.json `mounts`) for
# native ext4 build I/O instead of the slow 9p bind mount. A fresh volume
# mounts as root, so hand it to the non-root `vscode` user before any build
# writes to it. Idempotent: a no-op once the volume is already owned.
sudo mkdir -p /workspaces/oath/target
sudo chown vscode:vscode /workspaces/oath/target
```

- [ ] **Step 2: Verify the script still passes shellcheck**

Run: `shellcheck .devcontainer/post-create.sh`

Expected: no output, exit code 0.

- [ ] **Step 3: Verify the script is syntactically valid bash**

Run: `bash -n .devcontainer/post-create.sh`

Expected: no output, exit code 0.

- [ ] **Step 4: Commit**

```bash
git add .devcontainer/post-create.sh
git commit -m "chore(devcontainer): chown volume-backed target/ to vscode in post-create"
```

---

### Task 3: Rebuild the container and verify native-speed build I/O

This task is **user-driven**: rebuilding the devcontainer cannot be done from inside the running container. The agent prepares the verification commands; the user triggers the rebuild and runs them. The goal is to prove `target/` is no longer on 9p and that builds work as `vscode`.

**Files:**
- None (verification only).

**Interfaces:**
- Consumes: the `mounts` from Task 1 and the chown from Task 2.
- Produces: confirmation that the volumes are active and writable; no code artifacts.

- [ ] **Step 1 (optional baseline, BEFORE rebuild): record current 9p build time**

Run in the current (pre-change) container:

```bash
mount | grep '/workspaces/oath ' ; cargo build -q 2>/dev/null; time cargo build -q
```

Expected: the mount line shows `type 9p`; note the `real` time of the second (incremental, no-op) build for comparison. Skip if you don't want to wait — the mount-type check in Step 3 is the definitive proof.

- [ ] **Step 2: Rebuild the container**

In VS Code: open the Command Palette and run **Dev Containers: Rebuild Container**. Wait for `post-create.sh` to finish (it reinstalls apt + cargo tools; the cargo volume makes subsequent rebuilds faster).

- [ ] **Step 3: Verify `target/` is on a native volume, not 9p**

Run in a fresh container terminal:

```bash
mount | grep '/workspaces/oath/target'
```

Expected: a line for `/workspaces/oath/target` whose `type` is **not** `9p` (it will be the Docker volume's backing fs, e.g. `ext4`/`overlay`). If no line appears, the volume didn't mount — re-check the `mounts` block from Task 1.

- [ ] **Step 4: Verify the `vscode` user can write to `target/`**

Run:

```bash
whoami && touch /workspaces/oath/target/.write-test && rm /workspaces/oath/target/.write-test && echo "writable"
```

Expected: prints `vscode` then `writable`, with no `Permission denied`. A permission error means Task 2's chown didn't run — check `post-create.sh`.

- [ ] **Step 5: Verify the cargo cache volume is mounted and writable**

Run:

```bash
mount | grep '/usr/local/cargo' && touch /usr/local/cargo/.write-test && rm /usr/local/cargo/.write-test && echo "cargo cache ok"
```

Expected: a mount line for `/usr/local/cargo` (non-9p) followed by `cargo cache ok`.

- [ ] **Step 6: Verify a real build runs on the volume**

Run:

```bash
time just build
```

Expected: the build completes successfully writing into the volume-backed `target/`. A clean build is the fair comparison to the baseline; on this hardware/IO it should be materially faster than the 9p baseline from Step 1. (Incremental builds also benefit.)

- [ ] **Step 7 (optional): reclaim the now-shadowed Windows `target/`**

The old 5.8 GB `target/` still occupies space on `D:\` but is shadowed by the volume mount. To reclaim Windows disk space, from a **Windows** shell (not the container) delete `D:\...\oath\target`. Do **not** delete from inside the container — that would hit the volume, not the Windows directory.

---

## Self-Review

**1. Spec coverage:** The requirement — "improve performance without losing the mounted Windows share" — is met: source stays on the `D:\` bind mount (Task 1 touches only `target/` and `CARGO_HOME`); the two hot dirs move to native volumes (Task 1); the permission gotcha for the fresh `target` volume is fixed (Task 2); and the win is verified end-to-end (Task 3). No spec requirement is left unaddressed.

**2. Placeholder scan:** No TBD/TODO/"add error handling"/"similar to Task N" placeholders. Every code/edit step shows exact content; every command shows expected output.

**3. Type/identifier consistency:** Volume sources `oath-target-${devcontainerId}` and `oath-cargo-cache-${devcontainerId}`, and mount targets `/workspaces/oath/target` and `/usr/local/cargo`, are used identically across Tasks 1, 2, and 3. The chown path in Task 2 matches the mount target in Task 1. Verification commands in Task 3 reference the same paths.
