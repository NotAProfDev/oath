# Coding Standards Audit — 2026-06-21

Benchmarks each repo config against current upstream standards (2026).
Verdicts: **aligned** | **gap** (will fix in plan Task 8) | **intentional deviation** (documented, kept).

---

## rustfmt.toml

Reference: [rustfmt Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) | [rustfmt docs](https://rust-lang.github.io/rustfmt/)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `edition` | `"2024"` | Match workspace edition | aligned | Matches `workspace.package.edition = "2024"` in Cargo.toml | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `newline_style` | `"Unix"` | `"Unix"` or `"Auto"` | aligned | Explicitly forces LF line endings; safe cross-platform convention | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `match_block_trailing_comma` | `true` | `true` | aligned | Recommended for consistency; prevents single-line block arm diffs | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `imports_granularity` | disabled (commented out) | `"Module"` (nightly only) | intentional deviation | stay stable-only per project decision; option requires nightly rustfmt | [rustfmt issue #4991](https://github.com/rust-lang/rustfmt/issues/4991) |
| `group_imports` | disabled (commented out) | `"StdExternalCrate"` (nightly only) | intentional deviation | stay stable-only per project decision; option requires nightly rustfmt | [rustfmt issue #5083](https://github.com/rust-lang/rustfmt/issues/5083) |
| `tab_spaces` | not set (default `4`) | `4` | aligned | Rust community default; consistent with style guide | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `max_width` | not set (default `100`) | `100` | aligned | Rust community default; matches style guide | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `use_small_heuristics` | not set (default `"Default"`) | `"Default"` | aligned | Standard heuristics for line width management | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |
| `reorder_imports` | not set (default `true`) | `true` | aligned | Alphabetical import sorting; reduces merge conflicts | [Configurations.md](https://github.com/rust-lang/rustfmt/blob/main/Configurations.md) |

---

## .clippy.toml

Reference: [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `allow-unwrap-in-tests` | `true` | `true` | aligned | Standard exemption: test code should not require `#[allow(clippy::unwrap_used)]` on every assertion | [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) |
| `allow-expect-in-tests` | `true` | `true` | aligned | Same rationale as `allow-unwrap-in-tests`; test panics are intentional | [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) |
| `allow-indexing-slicing-in-tests` | `true` | `true` | aligned | Allows `vec[0]` in tests without an `#[allow]` attribute; avoids noise | [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) |
| `msrv` | not set | Set to match `rust-version` (`"1.85"`) | gap | Missing MSRV declaration in `.clippy.toml` means Clippy cannot suppress lints that require a newer Rust version. Should set `msrv = "1.85"` to match `workspace.package.rust-version`. | [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) |
| `cognitive-complexity-threshold` | not set (default `25`) | `25` or lower for financial code | aligned | Default is acceptable; can tighten per-crate if needed | [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html) |

---

## Cargo.toml (lints / profiles / metadata)

Reference: [The Cargo Book — Workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html) | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) | [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)

### workspace.package (metadata)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `version` | `"0.1.0"` | Defined; semver | aligned | Present and valid | [Cargo manifest](https://doc.rust-lang.org/cargo/reference/manifest.html) |
| `edition` | `"2024"` | `"2024"` (latest stable) | aligned | Using Rust 2024 edition; best practice for new projects | [Cargo manifest](https://doc.rust-lang.org/cargo/reference/manifest.html) |
| `rust-version` | `"1.85"` | Pin to lowest toolchain version you support | aligned | MSRV declared; matches CI MSRV job | [Cargo manifest](https://doc.rust-lang.org/cargo/reference/manifest.html) |
| `license` | `"MIT OR Apache-2.0"` | Dual license for libraries | aligned | Standard Rust ecosystem dual license | [Rust API Guidelines C-PERMISSIVE](https://rust-lang.github.io/api-guidelines/necessities.html) |
| `repository` | `"https://github.com/NotAProfDev/oath"` | Present | aligned | Present | [Rust API Guidelines C-METADATA](https://rust-lang.github.io/api-guidelines/documentation.html) |
| `description` | `"A modular, backend-agnostic trading engine"` | Present, concise | aligned | Clear one-line description | [Rust API Guidelines C-METADATA](https://rust-lang.github.io/api-guidelines/documentation.html) |
| `resolver` | `"2"` | `"2"` (required for workspaces with 2021+ edition) | aligned | Resolver v2 is the default for edition 2021+ but explicit declaration is best practice | [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html) |

### workspace.lints

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `rust.unused_imports` | `"warn"` | `"warn"` | aligned | Standard hygiene lint | [workspace.lints RFC 3389](https://rust-lang.github.io/rfcs/3389-manifest-lint.html) |
| `rust.dead_code` | `"warn"` | `"warn"` | aligned | Catches unused items | [workspace.lints RFC 3389](https://rust-lang.github.io/rfcs/3389-manifest-lint.html) |
| `rust.missing_docs` | `"warn"` | `"warn"` | aligned | Enforces documentation; appropriate for a library workspace | [workspace.lints RFC 3389](https://rust-lang.github.io/rfcs/3389-manifest-lint.html) |
| `rust.rust_2018_idioms` | `"warn"` | `"warn"` | aligned | Good idiom enforcement even in 2024-edition code | [workspace.lints RFC 3389](https://rust-lang.github.io/rfcs/3389-manifest-lint.html) |
| `rust.missing_debug_implementations` | `"warn"` | `"warn"` | aligned | All public types should implement `Debug` | [Rust API Guidelines C-DEBUG](https://rust-lang.github.io/api-guidelines/fmt.html) |
| `clippy.all` | `"warn"` | `"warn"` | aligned | Enables all stable Clippy lints | [Clippy usage](https://doc.rust-lang.org/clippy/usage.html) |
| `clippy.pedantic` | `"warn"` | `"warn"` | aligned | Industry standard for quality Rust codebases | [Clippy usage](https://doc.rust-lang.org/clippy/usage.html) |
| `clippy.nursery` | `"warn"` | `"warn"` (with selective allows) | aligned | Enabled with known-noisy lints commented; acceptable approach | [Clippy usage](https://doc.rust-lang.org/clippy/usage.html) |
| `clippy.cargo` | `"warn"` | `"warn"` | aligned | Enables Cargo manifest lints | [Clippy usage](https://doc.rust-lang.org/clippy/usage.html) |
| `clippy.cargo_common_metadata` | `"allow"` (priority 1) | `"allow"` for unpublished crates | aligned | Internal workspace crates are not published to crates.io | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| `clippy.multiple_crate_versions` | `"allow"` (priority 1) | `"allow"` when transitive dups are unavoidable | aligned | Transitive version conflicts outside project control; deny in `deny.toml` is the right layer | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| `clippy.dbg_macro` | `"warn"` | `"warn"` or `"deny"` | aligned | Catches leftover debug prints | [Mastering Clippy](https://rust-trends.com/posts/mastering-clippy-elevating-your-rust-code-quality/) |
| `clippy.panic_in_result_fn` | `"warn"` | `"warn"` | aligned | Prevents silent panics in `Result`-returning functions | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| `clippy.unwrap_used` | `"warn"` | `"warn"` | aligned | Enforces explicit error handling | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| `clippy.expect_used` | `"warn"` | `"warn"` | aligned | Enforces explicit error handling | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| `clippy.indexing_slicing` | `"warn"` | `"warn"` | aligned | Prevents panic-prone indexing | [Clippy lints](https://doc.rust-lang.org/stable/clippy/lints.html) |
| Member crate `[lints] workspace = true` | present in all members | required for inheritance | aligned | All crates inherit workspace lints | [workspace.lints RFC 3389](https://rust-lang.github.io/rfcs/3389-manifest-lint.html) |

### profiles

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `profile.release.lto` | `"fat"` | `"fat"` or `"thin"` for max perf | aligned | Fat LTO for maximum optimization in production | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.release.codegen-units` | `1` | `1` (required for fat LTO) | aligned | Single unit required for full LTO effectiveness | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.release.panic` | `"abort"` | `"abort"` for production | aligned | Eliminates unwinding overhead in production builds | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.release.strip` | `"symbols"` | `"symbols"` for binary size | aligned | Strips all symbols for minimal production binary | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.dev` | not set (Cargo defaults) | Cargo defaults acceptable | aligned | Defaults (opt-level=0, debug=true, incremental=true) are ideal for development | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.profiling` | defined (inherits release + debug=true) | Custom profiling profile with debug info | aligned | Industry practice: release perf + debug symbols for profilers | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.bench` | defined (inherits release + debug=true) | Custom bench profile with debug info | aligned | Enables flamegraph-readable stack traces during benchmarks | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| `profile.release.overflow-checks` | not set (default `false` in release) | Consider `true` for financial math | gap | Financial/trading engine should consider enabling overflow checks in release; arithmetic panics can be silent bugs without them. Enable per-crate or globally per risk assessment. | [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |

---

## deny.toml

Reference: [cargo-deny book](https://embarkstudios.github.io/cargo-deny/checks/index.html) | [advisories cfg](https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html) | [bans cfg](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html)

### [graph]

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `all-features` | `true` | `true` | aligned | Checks all feature combinations; prevents missed vulnerabilities in optional features | [cargo-deny graph](https://embarkstudios.github.io/cargo-deny/checks/cfg.html) |

### [advisories]

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `yanked` | `"deny"` | `"deny"` | aligned | Yanked versions should never be in a lockfile | [cargo-deny advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html) |
| `unmaintained` | `"workspace"` | `"workspace"` or `"all"` | aligned | Fail on unmaintained advisories for direct workspace deps; transitive deps excluded to reduce noise | [cargo-deny advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html) |
| `vulnerability` / `unsound` / `notice` | not set (removed in v0.16.0) | not applicable (deny by default) | aligned | These fields were removed in cargo-deny v0.16.0; all vulnerability/unsound/notice advisories now deny by default | [cargo-deny CHANGELOG](https://github.com/EmbarkStudios/cargo-deny/blob/main/CHANGELOG.md) |
| `ignore` | `[]` (empty) | Keep empty; use sparingly with documented reasons | aligned | No suppressed advisories; exemplary pattern with comment guidance | [cargo-deny advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html) |

### [licenses]

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `confidence-threshold` | `0.93` | `0.8` (default) or higher | aligned | Stricter-than-default threshold reduces false license approvals | [cargo-deny licenses](https://embarkstudios.github.io/cargo-deny/checks/cfg.html) |
| `allow` list | MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, ISC, BSD-2/3-Clause, Zlib, Unicode-3.0, Unicode-DFS-2016, CC0-1.0, 0BSD | Permissive SPDX list covering Rust ecosystem | aligned | Comprehensive permissive license allowlist; no copyleft | [SPDX license list](https://spdx.org/licenses/) |
| `licenses.private.ignore` | `true` | `true` for internal-only crates | aligned | Workspace crates are not published; skipping license check is correct | [cargo-deny licenses](https://embarkstudios.github.io/cargo-deny/checks/cfg.html) |

### [bans]

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `wildcards` | `"deny"` | `"deny"` | aligned | Wildcard version requirements are unacceptable in production | [cargo-deny bans](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html) |
| `highlight` | `"all"` | `"all"` | aligned | Report all duplicate instances for complete visibility | [cargo-deny bans](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html) |
| `multiple-versions` | not set (default `"warn"`) | `"warn"` or `"deny"` | aligned | Default warn is acceptable; bans.deny and bans.skip manage specific cases | [cargo-deny bans](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html) |
| `deny` | `[]` (empty) | Define per-project policy | aligned | No banned crates currently; ready for additions | [cargo-deny bans](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html) |
| `skip` | `[{ name = "wit-bindgen" }]` | Use `skip` with version constraint when possible | gap | `skip` entry for `wit-bindgen` lacks a version constraint; using `{ name = "wit-bindgen", version = "..." }` would allow cargo-deny to warn when the entry is no longer needed. Low priority. | [cargo-deny bans](https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html) |

### [sources]

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `unknown-registry` | `"deny"` | `"deny"` | aligned | Reject unrecognized registries | [cargo-deny sources](https://embarkstudios.github.io/cargo-deny/checks/cfg.html) |
| `unknown-git` | `"deny"` | `"deny"` | aligned | Reject raw git dependencies | [cargo-deny sources](https://embarkstudios.github.io/cargo-deny/checks/cfg.html) |

---

## rust-toolchain.toml

Reference: [rustup overrides / toolchain file](https://rust-lang.github.io/rustup/overrides.html) | [Should I pin my Rust toolchain version?](https://swatinem.de/blog/rust-toolchain/)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `channel` | `"stable"` (floating) | Pin exact version (e.g. `"1.87.0"`) for reproducibility | intentional deviation | Track latest stable per project decision; avoids manual pin-bump churn while still gating nightly usage. Floating stable is a documented trade-off. | [rustup overrides](https://rust-lang.github.io/rustup/overrides.html) |
| `components` | `["rustfmt", "clippy", "rust-analyzer"]` | Include all tools required by CI and dev workflow | aligned | All required tools declared; matches CI workflow components | [rustup toolchains](https://rust-lang.github.io/rustup/concepts/toolchains.html) |
| `profile` | `"minimal"` | `"minimal"` (avoid `"default"` in CI) | aligned | Minimal profile avoids installing unused components (docs, etc.) | [rustup toolchains](https://rust-lang.github.io/rustup/concepts/toolchains.html) |
| `targets` | not set | Set only when cross-compiling | aligned | No cross-compilation targets required; omission is correct | [rustup overrides](https://rust-lang.github.io/rustup/overrides.html) |

---

## _typos.toml

Reference: [typos reference](https://github.com/crate-ci/typos/blob/master/docs/reference.md) | [typos README](https://github.com/crate-ci/typos)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `files.extend-exclude` | `["Cargo.lock", "target/"]` | Exclude generated/vendored files | aligned | Correct exclusions; avoids false positives in generated lock file and build output | [typos reference](https://github.com/crate-ci/typos/blob/master/docs/reference.md) |
| `default.extend-words` | `{ oath = "oath" }` | Map project-specific terms to themselves | aligned | Correct pattern for false-positive suppression of the project name acronym | [typos reference](https://github.com/crate-ci/typos/blob/master/docs/reference.md) |
| CI integration | absent from `ci.yml` and `Justfile` | `typos` should run in CI and pre-commit hooks | gap | typos is configured but not wired into `just ci` or `.github/workflows/ci.yml`. A `just typos` recipe and CI step are missing. | [typos README](https://github.com/crate-ci/typos) |
| Pre-commit hook | absent (.githooks/ does not exist) | Run `typos` in pre-commit hook | gap | No git hooks directory exists; `just setup` configures `core.hooksPath` but `.githooks/` is empty. Pre-commit hook to run `typos` (and `cargo fmt --check`) is missing. | [typos README](https://github.com/crate-ci/typos) |

---

## Justfile / CI / git hooks

Reference: [just manual](https://just.systems/man/en/) | [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/) | [GitHub Actions security hardening](https://docs.github.com/en/actions/reference/security/secure-use)

### Justfile

| Setting / Recipe | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| Default recipe | `[private] default: @just --list` | List all recipes on bare `just` invocation | aligned | Correct ergonomic default | [just manual](https://just.systems/man/en/) |
| `fmt` recipe | `cargo fmt --all -- --check` | `cargo fmt --all -- --check` | aligned | Correct format check command | [just manual](https://just.systems/man/en/) |
| `check` recipe | `cargo check --workspace --all-targets --all-features` | Include `--workspace --all-targets --all-features` | aligned | Comprehensive type-check across all targets | [just manual](https://just.systems/man/en/) |
| `lint` recipe | `cargo clippy ... -- -D warnings` | `-D warnings` to hard-fail on lint | aligned | Warnings as errors; matches CI behavior | [just manual](https://just.systems/man/en/) |
| `test` recipe | `cargo test --workspace --all-features` | Include `--workspace --all-features` | aligned | Full test suite coverage | [just manual](https://just.systems/man/en/) |
| `deny` recipe | `cargo deny --all-features check` | Include `--all-features` | aligned | Matches `deny.toml` `graph.all-features = true` | [just manual](https://just.systems/man/en/) |
| `doc` recipe | `RUSTDOCFLAGS="-D warnings" cargo doc ...` | `-D warnings` via env var | aligned | Catches broken intra-doc links | [just manual](https://just.systems/man/en/) |
| `msrv` recipe | `cargo +1.85 check ...` | Explicit toolchain check | aligned | MSRV verification via `cargo +<version>` | [just manual](https://just.systems/man/en/) |
| `ci` aggregate | `ci: fmt lint test deny doc` | Should match CI workflow steps | gap | `just ci` omits `check` and `typos` steps that exist (check) or are planned (typos). CI workflow runs `check` separately; `just ci` does not, creating a divergence between local and remote CI. | [just manual](https://just.systems/man/en/) |
| `typos` recipe | absent | `typos` recipe for spell-check | gap | No `just typos` recipe; typos is configured in `_typos.toml` but unreachable via the task runner | [typos README](https://github.com/crate-ci/typos) |
| `setup` recipe | `git config core.hooksPath .githooks` | Configure hooks path | aligned | Correct one-time setup command | [just manual](https://just.systems/man/en/) |

### GitHub Actions (ci.yml)

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `permissions.contents` | `read` (job-level, both jobs) | Minimal token permissions | aligned | Least-privilege: contents:read is the minimum needed for checkout | [GitHub Actions security hardening](https://docs.github.com/en/actions/reference/security/secure-use) |
| `concurrency` (cancel-in-progress) | defined at workflow level | Cancel in-progress for same branch | aligned | Prevents redundant CI runs on fast-moving PRs | [GitHub Actions docs](https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions) |
| `CARGO_TERM_COLOR: always` | set at job level | `always` for readable CI logs | aligned | ANSI color in logs is standard practice | [Cargo environment](https://doc.rust-lang.org/cargo/reference/environment-variables.html) |
| Action version pinning (`actions/checkout@v6`) | mutable tag (`@v6`) | Pin to full commit SHA | gap | All four external actions (`actions/checkout@v6`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `actions/upload-artifact@v7`, `EmbarkStudios/cargo-deny-action@v2`) use mutable version tags. Best practice is to pin to a full commit SHA with a version comment. | [GitHub Actions security hardening](https://docs.github.com/en/actions/reference/security/secure-use) |
| `actions/checkout` version | `@v6` | Latest stable (v4 is widely used; v6 is current) | aligned | Using latest major version | [actions/checkout releases](https://github.com/actions/checkout/releases) |
| `Swatinem/rust-cache` version | `@v2` | `@v2` (current) | aligned | Current stable release | [Swatinem/rust-cache releases](https://github.com/swatinem/rust-cache/releases) |
| `EmbarkStudios/cargo-deny-action` version | `@v2` | `@v2` (current) | aligned | Current major version | [cargo-deny-action releases](https://github.com/EmbarkStudios/cargo-deny-action/releases) |
| `actions/upload-artifact` version | `@v7` | `@v7` (current) | aligned | Current major version | [actions/upload-artifact releases](https://github.com/actions/upload-artifact/releases) |
| Artifact `retention-days` | `7` | Less than default 90 days | aligned | Avoids burning artifact quota; 7 days is appropriate for build docs | [GitHub Actions docs](https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions) |
| `RUSTFLAGS` in CI | absent (intentional) | Not set at workflow level | aligned | Workspace lints handle rustc warnings without busting Swatinem cache key | [Cargo environment](https://doc.rust-lang.org/cargo/reference/environment-variables.html) |
| `typos` CI step | absent | Add `typos` check step | gap | No typos check in CI; configured tool is not gating CI runs. Should add a `typos` step. | [typos README](https://github.com/crate-ci/typos) |
| MSRV job `msrv` recipe | CI uses raw `cargo check` (not `just msrv`) | `just msrv` to stay in sync | gap | MSRV job in CI runs `cargo check` directly rather than delegating to `just msrv`. If the `msrv` recipe changes, CI may drift. | [just manual](https://just.systems/man/en/) |

### git hooks

| Setting | Our value | Best-practice value | Verdict | Action / rationale | Source |
|---|---|---|---|---|---|
| `.githooks/` directory | absent (deleted; `setup` recipe configures path) | pre-commit hook running fmt + lint + typos | gap | The `.githooks/` directory was removed (commit `3e60f53`). `just setup` points git at `.githooks/` but the directory and hooks do not exist. Pre-commit hook catching `cargo fmt --check` and `typos` is missing. | [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) |
| `commit-msg` hook (Conventional Commits) | absent | Validate commit messages against Conventional Commits 1.0.0 | gap | No commit-msg hook to enforce Conventional Commits format. Project commits follow the convention but it is not machine-enforced. | [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/) |

---

## Summary

### Gaps to fix in Task 8

1. **`.clippy.toml` — missing `msrv`**: Add `msrv = "1.85"` to align with `workspace.package.rust-version`.
2. **`Cargo.toml` — `profile.release.overflow-checks`**: Consider enabling `overflow-checks = true` in release for a financial/trading engine (evaluate trade-off vs. performance).
3. **`deny.toml` — `bans.skip` version constraint**: Add version constraint to `{ name = "wit-bindgen" }` skip entry so cargo-deny can warn when the entry becomes stale.
4. **`_typos.toml` / `Justfile` — typos not wired into CI or task runner**: Add `just typos` recipe and a `typos` step in `ci.yml`; include `typos` in `just ci` aggregate.
5. **`Justfile` — `just ci` diverges from remote CI**: Add `check` step to `just ci` (and `typos` once wired); `just ci` should mirror `ci.yml` exactly.
6. **`ci.yml` — action versions use mutable tags**: Pin all five external actions to full commit SHAs with version comments (supply-chain hardening).
7. **`ci.yml` — MSRV job uses raw `cargo check`**: Replace with `just msrv` so local and CI MSRV checks stay in sync.
8. **`git hooks` — `.githooks/` directory absent**: Recreate `.githooks/` with at minimum a `pre-commit` hook running `cargo fmt --check` and `typos`, and a `commit-msg` hook enforcing Conventional Commits format.

### Intentional deviations (kept)

- **Floating stable toolchain** (`rust-toolchain.toml channel = "stable"`): Tracks latest stable Rust instead of pinning an exact version. Avoids manual pin-bump churn. Documented project decision; CI cache is keyed on toolchain file.
- **Stable-only rustfmt** (`imports_granularity` and `group_imports` disabled): Both options require nightly rustfmt. Project decision to stay on stable rustfmt only. Options are commented with tracking issues (#4991, #5083) for future re-evaluation.
