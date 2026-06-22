# Contributing to OATH

Thanks for your interest in OATH! This guide summarises how we work. For the
authoritative, always-current details, see [CLAUDE.md](CLAUDE.md) and
[README.md](README.md) — this document points back to them rather than
duplicating them.

> ⚠️ OATH is **pre-release — do not use in production.** It is still being built.

## Development workflow — one issue, one PR

1. **Issue** — open a GitHub issue using the [issue templates](.github/ISSUE_TEMPLATE)
   (bug, feature, or question).
2. **Branch** — branch off `main` for that one issue: `feat/<slug>` or
   `fix/<slug>`. One branch implements one issue.
3. **Implement** — make the change. For non-trivial work, write a spec then a
   plan first (under `docs/superpowers/`).
4. **Local CI** — `just ci` must pass. The pre-push hook enforces the full gate,
   so a clean push means local CI is green.
5. **Pull request** — open a PR that references the issue (`Closes #N`) and fill
   in the [PR template](.github/PULL_REQUEST_TEMPLATE.md).
6. **Cloud CI** — GitHub Actions runs `just ci` plus the MSRV job; it must be
   green to merge.
7. **Squash + merge** — the PR is squash-merged into `main`; the issue closes.

## Getting set up

- **Dev container (recommended):** open the repo in the dev container; it
  provisions all tooling (`gh`, `just`, the `cargo-*` tools, `gitleaks`,
  `shellcheck`, `actionlint`, `typos`, `taplo`) and wires the git hooks for you.
- **Local clone:** run `just setup` once to point Git at `.githooks` and make the
  hooks executable. Run `just --list` to see every recipe.

The Rust toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) — Rust
`1.96.0` with the `rustfmt`, `clippy`, and `rust-analyzer` components. `rustup`
installs it automatically the first time you run a `cargo` command in the repo, so
there's no manual setup. Treat a version bump as a deliberate, reviewed change, and
keep it in sync with the devcontainer base image (`rust:<version>-trixie`) and the
`dtolnay/rust-toolchain` ref in CI.

## Commands

`just` is the single entry point — prefer a recipe over raw `cargo`, because the
recipes pin the exact flags CI uses. Run `just --list` for the full set.

| Task | Command |
| --- | --- |
| Type-check everything | `just check` |
| Run tests (+ doctests) | `just test` |
| Lint (warnings = errors) | `just lint` |
| Dependency supply-chain gate | `just deny` |
| Build docs (warnings = errors) | `just doc` |
| Spell-check | `just typos` |
| Mutation testing (slow) | `just mutants` (or `just mutants-diff` for changed lines) |
| Auto-fix fmt + clippy | `just fix` |
| Full local CI suite | `just ci` |

`just mutants` is part of the workflow on purpose: it checks that tests *catch*
introduced bugs, not just that lines are covered. A full run is slow, so `just ci`
leaves it out — run it separately, or `just mutants-diff` for the changed-lines
loop.

## Code conventions

Hard rules, enforced workspace-wide by `[workspace.lints]` in the root
`Cargo.toml` (see [CLAUDE.md](CLAUDE.md) for the full list):

- **No `unsafe`** — FFI needs a per-crate override with justification.
- **No `unwrap` / `expect` / indexing** in non-test code — return `Result` and
  model errors with `thiserror`. Test code is exempt (configured in
  [`clippy.toml`](clippy.toml)).
- **Document public items** — `missing_docs` is on.
- **Respect the dependency direction** in the README graph — never introduce a
  cycle.
- Edition **2024**, MSRV **1.90** (validate with `just msrv`).

Lints are single-sourced: the table lives in the root `Cargo.toml`
(`[workspace.lints]`) and the configurable knobs in [`clippy.toml`](clippy.toml),
both inherited by every crate. Escape a lint **narrowly and with a reason**, never
blanket-disabled:

```rust
#[allow(clippy::some_lint, reason = "why this case is legitimately fine")]
```

If a crate genuinely needs `unsafe` (e.g. FFI), override it in *that crate's*
`Cargo.toml` with a justifying comment rather than relaxing the workspace default:

```toml
[lints.rust]
unsafe_code = "allow"   # which FFI boundary, and why it's sound
```

Work is not done until `just ci` passes — identical to the cloud CI gate. Don't
bypass the git hooks.

## Dependencies

`cargo deny` is the supply-chain gate, run via `just deny`. Policy lives in
[`deny.toml`](deny.toml) at the workspace root and covers four checks:

| Check | What it enforces |
| --- | --- |
| `advisories` | No crates with RustSec security advisories; yanked versions are an error. |
| `licenses` | Every dependency's license is on the permissive allow-list; copyleft is denied unless deliberately added. |
| `bans` | No wildcard (`*`) version requirements; duplicate versions warn. |
| `sources` | Crates may only come from crates.io or explicitly whitelisted git remotes. |

Pulling in a dependency with a new license, or one that trips an advisory, is a
**deliberate, reviewed decision** — add an `allow` / `exceptions` / `ignore` entry
to `deny.toml` *with a justification comment*, never to silence the gate casually.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org), enforced by
the `commit-msg` hook. The subject is `<type>(<optional-scope>)!: <description>`,
capped at 72 characters, with an imperative description:

```text
feat(engine): add sliding-window order matcher
fix(risk): reject negative position limits
chore(deps): bump thiserror to 2.0
```

| Type | Use for |
| --- | --- |
| `feat` | A new capability. |
| `fix` | A bug fix. |
| `docs` | Documentation only. |
| `refactor` | A behaviour-preserving code change. |
| `perf` | A performance improvement. |
| `test` | Adding or fixing tests. |
| `build` / `ci` | Build system / CI configuration. |
| `chore` | Tooling, deps, housekeeping. |
| `style` | Formatting only (no logic change). |
| `revert` | Reverting a previous commit. |

Scope is free-form lowercase — prefer the crate or area you touched (e.g.
`engine`, `risk`, `deps`).

## Git hooks

Three hooks live in `.githooks/` and run automatically once `just setup` has
wired `core.hooksPath` (the dev container does this on create). Each is a thin
wrapper around a `just` recipe, and they **fail closed**: if `just` isn't
installed the hook refuses the operation rather than skipping the checks — install
`just` or re-enter the dev container.

| Hook | Recipe | What it gates |
| --- | --- | --- |
| `pre-commit` | `just pre-commit` | No commits on a protected branch (`main`), merge-conflict markers, oversized files (> 5 MiB), `fmt` + `fmt-toml` + `typos`, `lint` (clippy, warnings = errors), and `test-no-run` — tests must *build*, but aren't *run* here. |
| `commit-msg` | `just commit-msg` | The subject line follows Conventional Commits (≤ 72 chars). |
| `pre-push` | `just pre-push` | No pushes targeting a protected branch, then the full `just ci` gate before anything leaves your machine. |

The hooks are a convenience, not a hard gate: `git commit`/`git push --no-verify`,
or a fresh clone that never ran `just setup`, walks straight past them. The same
gate runs in GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml))
on every PR, where it must be green to merge. Protected branches are
single-sourced in `PROTECTED_BRANCHES` in the `justfile`.

## Security

Found a vulnerability? Please follow [SECURITY.md](SECURITY.md) — report it
through GitHub private vulnerability reporting, not a public issue.
