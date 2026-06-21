# Contributing to OATH

Thanks for your interest in OATH! This guide summarises how we work. For the
authoritative, always-current details, see [CLAUDE.md](../CLAUDE.md) and
[README.md](../README.md) — this document points back to them rather than
duplicating them.

> ⚠️ OATH is **pre-release — do not use in production.** It is still being built.

## Development workflow — one issue, one PR

1. **Issue** — open a GitHub issue using the [issue templates](ISSUE_TEMPLATE)
   (bug, feature, or question).
2. **Branch** — branch off `main` for that one issue: `feat/<slug>` or
   `fix/<slug>`. One branch implements one issue.
3. **Implement** — make the change. For non-trivial work, write a spec then a
   plan first (under `../docs/superpowers/`).
4. **Local CI** — `just ci` must pass. The pre-push hook enforces the full gate,
   so a clean push means local CI is green.
5. **Pull request** — open a PR that references the issue (`Closes #N`) and fill
   in the [PR template](PULL_REQUEST_TEMPLATE.md).
6. **Cloud CI** — GitHub Actions runs `just ci` plus the MSRV job; it must be
   green to merge.
7. **Squash + merge** — the PR is squash-merged into `main`; the issue closes.

## Getting set up

- **Dev container (recommended):** open the repo in the dev container; it
  provisions all tooling (`gh`, `just`, the `cargo-*` tools, `gitleaks`,
  `shellcheck`, `actionlint`, `typos`, `taplo`) and wires the git hooks for you.
- **Local clone:** run `just setup` once to point Git at `.githooks` and make the
  hooks executable. Run `just --list` to see every recipe.

## Commands

`just` is the single entry point — prefer a recipe over raw `cargo`, because the
recipes pin the exact flags CI uses.

| Task | Command |
|---|---|
| Type-check everything | `just check` |
| Run tests (+ doctests) | `just test` |
| Lint (warnings = errors) | `just lint` |
| Auto-fix fmt + clippy | `just fix` |
| Full local CI suite | `just ci` |

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org), enforced by
the `commit-msg` hook — for example `feat(engine): …`, `fix(risk): …`,
`chore(deps): …`, `ci(actions): …`. Subjects are capped at 72 characters.

## Code conventions

Hard rules, enforced workspace-wide by `[workspace.lints]` (see
[CLAUDE.md](../CLAUDE.md) for the full list):

- **No `unsafe`** — FFI needs a per-crate override with justification.
- **No `unwrap` / `expect` / indexing** in non-test code — return `Result` and
  model errors with `thiserror`. Test code is exempt.
- **Document public items** — `missing_docs` is on.
- **Respect the dependency direction** in the README graph — never introduce a
  cycle.
- Edition **2024**, MSRV **1.85** (validate with `just msrv`).

Work is not done until `just ci` passes — identical to the cloud CI gate. Don't
bypass the git hooks.

## Security

Found a vulnerability? Please follow [SECURITY.md](SECURITY.md) — report it
through GitHub private vulnerability reporting, not a public issue.
