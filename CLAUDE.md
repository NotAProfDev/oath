# CLAUDE.md

Guidance for Claude Code working in **OATH** (Open Automatic Trading Hub),
a modular, backend-agnostic Rust trading engine. For architecture and the crate
dependency graph, see [README.md](README.md).

## Status: core trait crates defined, backends not yet built

The Cargo workspace and the ten trait-defining `*-core` crates (`oath-model` →
`oath-engine`) are in place, along with the full dev tooling (`justfile`,
`.githooks/`, lint config). Backend and adapter crates (e.g. `oath-net-reqwest`,
`oath-persistence-sqlite`, `oath-adapter-ibkr`) are **not yet built**. The project
is pre-release — see the "do not use" notice in the README.

## Development workflow

Every change follows this lifecycle — **one issue, one PR**:

1. **Issue** — open a GitHub issue describing the change (use the issue templates).
2. **Branch** — branch off `main` to implement that issue (e.g. `feat/<slug>`,
   `fix/<slug>`). One branch implements one issue.
3. **Local CI** — implement, then `just ci` must pass. The pre-push hook enforces
   the full gate, so a clean push means local CI is green.
4. **PR** — open a pull request that references the issue (`Closes #N`).
5. **Cloud CI** — GitHub Actions (`.github/workflows/ci.yml`) runs `just ci` and
   the MSRV job on the PR. It must be green to merge.
6. **Squash + merge** — squash-merge the PR into `main`; the issue closes.

For non-trivial work, write a spec → plan first (see Agent skills below); specs
and plans live in [docs/superpowers/](docs/superpowers/).

## Command interface

`just` is the single entry point. Prefer a `just` recipe over raw `cargo` —
the recipes pin the exact flags CI uses. Run `just --list` for the full set.

| Task | Command |
|---|---|
| Type-check everything | `just check` |
| Run tests (+ doctests) | `just test` |
| Lint (warnings = errors) | `just lint` |
| Auto-fix fmt + clippy | `just fix` |
| Full local CI suite | `just ci` |

## Definition of done

Work is not done until **`just ci` passes** — identical to the GitHub Actions CI
gate (fmt, lint, test, doc, deny, typos, …). Don't bypass the git hooks. No new
warnings: clippy `all` is **deny-level**.

## Code conventions

Hard rules, enforced workspace-wide by `[workspace.lints]`:

- **No `unsafe`** — `unsafe_code = "deny"`; FFI needs a per-crate override with
  justification.
- **No `unwrap` / `expect` / indexing** in non-test code (warned) — return
  `Result`, model errors with `thiserror`. Test code is exempt.
- **Document public items** — `missing_docs` is warned.
- **Respect dependency direction** — `oath-model` is the root; never introduce a
  cycle or a dep that contradicts the README graph.
- Edition **2024**, MSRV **1.90** (validate with `just msrv`).

## Commits & labels

- **Conventional Commits**, enforced by the `commit-msg` hook — e.g.
  `feat(engine): …`, `fix(risk): …`, `chore(deps): …`, `ci(actions): …`.
- **Labels**: `bug`, `enhancement`, `question`, `needs-triage` (issues);
  `dependencies`, `ci` (Dependabot PRs). Reuse these — don't invent new ones ad hoc.

## Agent skills

This repo uses the **superpowers** plugin (enabled in `.claude/settings.json`,
alongside `rust-analyzer-lsp` and `claude-md-management`). Prefer its process
skills — `brainstorming`, `writing-plans`, `test-driven-development`,
`systematic-debugging` — for any non-trivial work.

Project-specific skills live in `.claude/skills/`: `ask-matt`, `codebase-design`,
`diagnosing-bugs`, `domain-modeling`, `grill-me`, `grill-with-docs`, `grilling`,
`handoff`, `implement`, `improve-codebase-architecture`, `prototype`,
`resolving-merge-conflicts`, `setup-matt-pocock-skills`, `tdd`, `to-issues`,
`to-prd`, `triage`.
