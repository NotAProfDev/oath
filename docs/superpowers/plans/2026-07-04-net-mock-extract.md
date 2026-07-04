# net-mock extraction (WS resilience PR0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Relocate `MockTimer` out of `oath-adapter-net-http-mock` into a new dev-only crate `oath-adapter-net-mock`, beside the `Timer` contract in `oath-adapter-net-api`, so the HTTP and (forthcoming) WebSocket mock stacks share one fake clock without dev-depending on each other's mock crate.

**Architecture:** A pure relocation — no behavior change. `MockTimer` moves verbatim (via `git mv`, preserving history) into the new crate; `oath-adapter-net-http-mock` keeps only `MockClient`/`MockBody`; the workspace gains one member. `MockTimer` has **no consumers today** (it was built ahead of the HTTP resilience layers), so nothing external needs repointing — the acceptance surface is "both mock crates still build and test green, `just machete` stays green, and the reachability guard holds."

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `cargo`. No new external dependencies — the new crate reuses the existing `oath-adapter-net-api` + `tokio` (dev) workspace deps.

**Source spec:** [docs/superpowers/specs/2026-07-04-net-ws-resilience-design.md](../specs/2026-07-04-net-ws-resilience-design.md) — this is **PR0** of that spec's PR map. Mandated by [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md) §Amendments.4, which relocates `MockTimer` to `oath-adapter-net-mock` expressly because "the WS resilience slice (ADR-0033 §9) is imminent."

## Global Constraints

Every task implicitly includes these (from CLAUDE.md, the workspace `[workspace.lints]`, and the spec):

- **Edition 2024, MSRV 1.90.** No `unsafe` — `unsafe_code = "deny"`; the new crate carries `#![forbid(unsafe_code)]` (as every mock crate does).
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result` / recover (the `lock` poison-recovery helper). Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` runs clippy with `-D warnings`** and promotes `pedantic`/`nursery` to errors — all code including tests must be pedantic-clean: `#[must_use]` where clippy asks, document all public items (`missing_docs`), `Debug` on all public types (`missing_debug_implementations`), no unreachable `pub`.
- **Deps** via `[workspace.dependencies]` (internal crates carry an explicit `version`).
- **DoD:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete, …). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree under `.claude/worktrees/net-mock-extract` (never switch the primary checkout) → one PR (`Closes #<issue>`).

---

## File Structure

- `crates/adapter/net/mock/Cargo.toml` — **new crate** `oath-adapter-net-mock`.
- `crates/adapter/net/mock/src/lib.rs` — **new.** Crate root: `MockTimer` re-export + the `lock` poison-recovery helper.
- `crates/adapter/net/mock/src/timer.rs` — **moved** verbatim from `crates/adapter/net/http/mock/src/timer.rs`.
- `crates/adapter/net/http/mock/src/lib.rs` — **modify.** Drop the `timer` module + `MockTimer` re-export; update the module doc.
- `Cargo.toml` (workspace) — **modify.** Add the member + the `[workspace.dependencies]` entry.
- `CHANGELOG.md` — **modify.** `[Unreleased] → Changed`.
- **No README change** — the crate table + mermaid graph list only the `*-api` contract crates; the existing dev-only mock crates (`net-http-mock`, `net-ws-mock`) are already absent, so `net-mock` follows that established convention.

Two tasks: Task 1 is the relocation (one atomic, reviewable unit); Task 2 is the CHANGELOG + guard + gate + PR wrap.

---

## Setup: issue + worktree

- [ ] **Create the issue**

```bash
gh issue create \
  --title "refactor(net): extract MockTimer into shared oath-adapter-net-mock crate (WS resilience PR0)" \
  --label enhancement \
  --body "PR0 of the net-ws resilience surface (spec: docs/superpowers/specs/2026-07-04-net-ws-resilience-design.md), mandated by ADR-0034 §Amendments.4.

Relocate \`MockTimer\` from \`oath-adapter-net-http-mock\` into a new dev-only \`oath-adapter-net-mock\` crate beside the \`Timer\` contract in \`oath-adapter-net-api\`, so the HTTP and WS mock stacks share one fake clock without cross-depending. \`net-http-mock\` keeps only \`MockClient\`/\`MockBody\`."
```

Note the issue number `#<N>` for the PR body.

- [ ] **Confirm the worktree**

The isolated worktree already exists (created during planning) and holds the committed spec + this plan:

```bash
git worktree list | grep net-mock-extract
# .../.claude/worktrees/net-mock-extract  <sha> [refactor/net-mock-extract]
cd .claude/worktrees/net-mock-extract
```

All subsequent tasks run inside this worktree. (If it is missing, recreate it: `git worktree add .claude/worktrees/net-mock-extract -b refactor/net-mock-extract main`.)

---

## Task 1: Relocate `MockTimer` into `oath-adapter-net-mock`

**Files:**
- Create: `crates/adapter/net/mock/Cargo.toml`, `crates/adapter/net/mock/src/lib.rs`
- Move: `crates/adapter/net/http/mock/src/timer.rs` → `crates/adapter/net/mock/src/timer.rs`
- Modify: `crates/adapter/net/http/mock/src/lib.rs`, root `Cargo.toml`

**Interfaces:**
- Consumes: `oath_adapter_net_api::Timer` (the trait `MockTimer` implements); `tokio` (dev, for the moved tests).
- Produces: `oath_adapter_net_mock::MockTimer` — the identical public API as before (`MockTimer::new()`, `Default`, `advance(&self, Duration)`, `impl Timer`), now importable from the transport-neutral crate. PR1 (`net-ws-mock`'s `MockSpawn`) and the future HTTP resilience layers dev-depend on it here.

- [ ] **Step 1: Register the new crate in the workspace**

In the root `Cargo.toml`, add the member directly after the `net/api` entry (keeping the net crates grouped):

```toml
  "crates/adapter/net/api",
  "crates/adapter/net/mock",
```

and the internal-crate dependency directly after the `oath-adapter-net-api` entry in `[workspace.dependencies]`:

```toml
oath-adapter-net-api = { path = "crates/adapter/net/api", version = "0.1.0" }
oath-adapter-net-mock = { path = "crates/adapter/net/mock", version = "0.1.0" }
```

- [ ] **Step 2: Create the new crate's `Cargo.toml`**

Create `crates/adapter/net/mock/Cargo.toml`:

```toml
[package]
name = "oath-adapter-net-mock"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
oath-adapter-net-api = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: Create the new crate root `src/lib.rs`**

Create `crates/adapter/net/mock/src/lib.rs` (this also creates the `src/` dir the next step's `git mv` targets). It carries its own copy of the `lock` helper, exactly as `net-http-mock` and `net-ws-mock` each do — the moved `timer.rs` calls `crate::lock`:

```rust
//! Transport-neutral test doubles for the net adapter stack: a `MockTimer`
//! virtual clock beside the `Timer` contract in `oath-adapter-net-api`. Consumed
//! via `[dev-dependencies]` only — it has no production edge, so the HTTP and WS
//! stacks can fake the same clock without dev-depending on each other's mock.
#![forbid(unsafe_code)]

pub mod timer;

pub use timer::MockTimer;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state stays
/// usable so a failing test reports its own assertion, not a poison panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
```

- [ ] **Step 4: Move `timer.rs` verbatim (preserving history)**

```bash
git mv crates/adapter/net/http/mock/src/timer.rs crates/adapter/net/mock/src/timer.rs
```

Do **not** edit the file's contents — it already imports `crate::lock` and `oath_adapter_net_api::Timer`, both of which resolve identically in the new crate. (The verbatim move is why this is a `git mv`, not a re-paste: no transcription risk, history preserved.)

- [ ] **Step 5: Drop `MockTimer` from `net-http-mock`**

Edit `crates/adapter/net/http/mock/src/lib.rs` — remove the `timer` module line, remove the `MockTimer` re-export, and update the module doc. The full new file:

```rust
//! Test harness for the net-http stack: a canned-response `MockClient` leaf and
//! a frame-controllable `MockBody`. Consumed by downstream crates via
//! `[dev-dependencies]` only — it has no production edge. (The `MockTimer`
//! virtual clock now lives in the transport-neutral `oath-adapter-net-mock`.)
#![forbid(unsafe_code)]

pub mod body;
pub mod client;

pub use body::MockBody;
pub use client::MockClient;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state stays
/// usable so a failing test reports its own assertion, not a poison panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
```

(`lock` stays — `client.rs` still uses it. `tokio` + `http-body-util` dev-deps stay — `body.rs`/`client.rs` tests still use them. So `net-http-mock/Cargo.toml` is unchanged and `just machete` stays green.)

- [ ] **Step 6: Verify both crates build and test green**

Run: `cargo test -p oath-adapter-net-mock -p oath-adapter-net-http-mock`
Expected: PASS — the moved timer tests (`repeated_poll_does_not_stack_waiters`, `advance_moves_now_and_wakes_sleepers`) run under `oath-adapter-net-mock`; `net-http-mock`'s `MockClient`/`MockBody` tests still pass with no `MockTimer`.

- [ ] **Step 7: Verify lint + machete are clean**

Run: `just lint && just machete`
Expected: PASS — no clippy warnings; no unused dependency (in particular `net-http-mock`'s `tokio`/`http-body-util` remain used by `body.rs`/`client.rs`; `net-mock`'s `oath-adapter-net-api` is used by `timer.rs` and `tokio` by its tests).

- [ ] **Step 8: Commit**

```bash
git add crates/adapter/net/mock crates/adapter/net/http/mock/src/lib.rs Cargo.toml Cargo.lock
git commit -m "refactor(net): extract MockTimer into shared oath-adapter-net-mock crate"
```

---

## Task 2: Reachability guard, CHANGELOG, full gate, PR

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Assert the dev-only reachability guard**

Both mock crates must stay unreachable from production code (ADR-0034 §Amendments.4).

Run: `cargo tree -e no-dev -i oath-adapter-net-mock`
Expected: **no non-dev dependent.** At PR0 nothing depends on `net-mock` yet (its first dev-dependent, `net-ws-mock`, arrives in PR1), so the command prints only the crate itself with no parent lines. If any crate appears as a dependent under `-e no-dev`, that is a production leak — stop and fix before proceeding.

- [ ] **Step 2: Update the CHANGELOG**

Add to `CHANGELOG.md` under `## [Unreleased]` → `### Changed` (as the last bullet of that subsection):

```markdown
- `MockTimer` relocated from `oath-adapter-net-http-mock` into a new dev-only
  `oath-adapter-net-mock` crate beside the `Timer` contract in
  `oath-adapter-net-api`, so the HTTP and (forthcoming) WebSocket mock stacks
  share one fake clock without cross-depending (ADR-0034 §Amendments.4).
  `oath-adapter-net-http-mock` now provides only `MockClient`/`MockBody`.
```

- [ ] **Step 3: Run the full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete, …).

- [ ] **Step 4: Commit, push, PR**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): net-mock extraction (WS resilience PR0)"
git push -u origin refactor/net-mock-extract
gh pr create \
  --title "refactor(net): extract MockTimer into shared oath-adapter-net-mock crate (WS resilience PR0)" \
  --body "Closes #<N>

PR0 of the net-ws resilience surface (spec: docs/superpowers/specs/2026-07-04-net-ws-resilience-design.md), mandated by ADR-0034 §Amendments.4.

- New dev-only crate \`oath-adapter-net-mock\` holding \`MockTimer\`, beside the \`Timer\` contract in \`oath-adapter-net-api\`.
- \`MockTimer\` moved verbatim (history preserved via \`git mv\`); \`oath-adapter-net-http-mock\` now provides only \`MockClient\`/\`MockBody\`.
- Lets the HTTP and (forthcoming) WS mock stacks share one fake clock without a WS-mock → HTTP-mock cross-dependency.
- No behavior change; \`MockTimer\` had no consumers yet, so nothing external is repointed. Both mock crates keep the \`cargo tree -e no-dev -i\` production-reachability guard.

This PR also lands the WS resilience design spec (docs/superpowers/specs/2026-07-04-net-ws-resilience-design.md) and this plan.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (PR0 in the spec's PR map):**
- Create `oath-adapter-net-mock`, move `MockTimer` in, `net-http-mock` keeps only `MockClient`/`MockBody` — Task 1. ✅
- Repoint consumers — **none exist** (`MockTimer` is unconsumed today), so nothing to repoint; acceptance is HTTP-tests-green — Task 1 Step 6. ✅
- Production-reachability guard (`cargo tree -e no-dev -i`) — Task 2 Step 1. ✅
- Workspace member + `[workspace.dependencies]` entry — Task 1 Step 1. ✅
- README — deliberately unchanged (mock crates are absent by convention); noted in File Structure. ✅
- CHANGELOG + `just ci` + one-issue-one-PR mechanics — Task 2. ✅
- Amends ADR-0033 §9 (`MockTimer` home) via ADR-0034 §Amendments.4 — cited in the header + CHANGELOG. ✅

**Placeholder scan:** none — every code step carries the actual file content or the exact `git mv`; every run step the exact command + expected result. (`#<N>` is the standard issue-number substitution, filled at Setup.)

**Type consistency:** `MockTimer`'s public surface (`new`, `Default`, `advance`, `impl Timer`) is unchanged by a verbatim `git mv`; `crate::lock` resolves in `net-mock` because Step 3's `lib.rs` defines it; `oath-adapter-net-api`/`tokio` are the only deps `timer.rs` needs and both are declared in Step 2's `Cargo.toml`. The `net-http-mock` `lib.rs` rewrite in Step 5 keeps `lock`, `body`, `client` — all still used.

**Known risks to watch during impl:**
- `git mv` requires the destination `src/` dir to exist — Step 3 (creating `lib.rs`) precedes Step 4, so it does.
- If `Cargo.lock` is committed in this repo, Step 8 stages it; if the repo `.gitignore`s it, the `git add Cargo.lock` is a harmless no-op.
- `cargo tree -i` on a crate with zero dependents prints just the crate node — that is the guard passing, not an error.
</content>
