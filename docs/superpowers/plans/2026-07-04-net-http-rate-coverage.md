# net-http RateKey + RateLimitConfig + Boot-Time Coverage (Slice 0, PR 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the boot-time pacing-coverage contract in `oath-adapter-net-http-api` — the `RateKey` trait, the `LimitPolicy`/`LimitDecl` vocabulary, the total `RateLimitConfig<K>` map, `BuildError`, and the standalone `validate_coverage` validator — so a missing or ill-configured pacing bucket is a **boot failure**, not a first-live-order 429 → IBKR penalty box. This closes Slice 0.

**Architecture:** `RateKey` is a trait whose implementors (an adapter's endpoint enum) expose a **finite universe** via `fn all() -> &'static [Self]`. `RateLimitConfig<K>` is a **total** map: every `K::all()` variant must be *explicitly classified* (`LimitDecl::Policy` or `LimitDecl::GlobalOnly` — never "absent"), plus a required `global` policy. `validate_coverage` is the pure construction-time check (totality + param sanity) returning `Result<(), BuildError>`; Slice 2's `stack()`/`build()` will call it, but it is complete and unit-tested standalone here. No layer, no `RateLimit` runtime, no `Scope` request-surface enforcement (those are Slice 1). Pure data + one validator — no async, no new dependency.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `std::collections::HashMap`, `thiserror` (already a crate dependency). **No new dependency** — `net-http-api` stays runtime-free (no `tokio`/`hyper`/`reqwest`/`serde`).

**Source spec:** [docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md](../specs/2026-06-30-net-http-construction-surface-design.md) §"`RateKey` — a typed enum with a finite universe" / §"Boot-time total pacing coverage" (lines ~298–363), recorded in [ADR-0034 §3](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). Roadmapped as **PR 4** in [2026-06-30-net-http-foundation.md](2026-06-30-net-http-foundation.md) (lines 435–441).

**Depends on PR 3 having merged** (it has — #66): PR 4 adds a new module to the same crate; it consumes only `thiserror` and `std`. It does not consume PR 3's `AuthSource`/`Guarded`.

## Decisions locked by this plan

1. **Scope = config + validator only.** `RateKey`, `LimitPolicy`, `LimitDecl`, `RateLimitConfig<K>`, `BuildError`, `validate_coverage`. **Deferred, correctly absent here:** the `RateLimit<K>` *layer*, the per-request `Scope`/`RateLimit<K>` extension and its call-site fail-closed (spec lines 342–357), `HttpConfig`, `stack()`/`build()` wiring — all Slice 1/Slice 2. This PR delivers the validator those slices call.

2. **`BuildError` is non-generic with two variants — `UndeclaredKey(String)` and `InvalidPolicy(String)`.** The spec sketch (line 362) also lists "missing global", but `RateLimitConfig<K>.global` is a **required, non-optional field** — "missing global" is structurally unrepresentable, so no `MissingGlobal` variant is added (a dead, unconstructible variant would be worse design and a `missing_docs`/clippy liability). The global policy's *params* are still validated, via the same `InvalidPolicy` path as any local policy. `BuildError` must be non-generic because Slice 2's `stack()`/`build()` return `Result<_, BuildError>` (spec lines 267/273), so the offending key is rendered to a `String`.

3. **`RateKey`'s supertraits stay exactly as the spec defines them** — `Hash + Eq + Clone + Send + Sync + 'static` (no `Debug` added to the trait). Only `validate_coverage` needs to *render* an undeclared key, so it — not the trait — carries the `K: fmt::Debug` bound. `RateLimitConfig<K>` derives `Debug` conditionally (`impl<K: Debug> Debug`), which satisfies the workspace `missing_debug_implementations` lint without forcing `Debug` onto every `RateKey`.

4. **`validate_coverage` is `pub`** — it is part of the boot-time contract (an adapter may pre-validate its pacing table), and Slice 2's `stack()`/`build()` (same crate) call it.

5. **Drift-proofing lives in the *test* `RateKey`, not the trait.** Per the spec (lines 316–322) the production trait stays dependency-free; the *adapter* owns `all()` exhaustiveness (via `strum::VariantArray` or an exhaustive-`match` test). PR 4's test key uses the dependency-free exhaustive-`match` guard so the pattern is demonstrated and pinned.

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` (`unsafe_code = "deny"`; the crate is `#![forbid(unsafe_code)]`).
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result`. Test code is exempt for `unwrap`/`expect`/indexing only.
- **`just lint` runs clippy with `-D warnings` and promotes `pedantic`/`nursery`** — all code including tests must be pedantic-clean: `#[must_use]` where clippy asks, document all public items (`missing_docs`), `Debug` on all public types (`missing_debug_implementations`), no unreachable `pub`, `const fn` where nursery's `missing_const_for_fn` asks. Compare unsigned to `== 0`, not `< 1` (clippy).
- **`net-http-api` charter:** free of any async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde`. **This PR adds no dependency** (`thiserror` and `std` only), so `cargo-machete` and `cargo-deny` are unaffected.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree under `.claude/worktrees/<slug>` → one PR (`Closes #<issue>`).

---

## File Structure

- `crates/adapter/net/http/api/src/rate.rs` — **new.** `RateKey`, `LimitPolicy`, `LimitDecl`, `RateLimitConfig<K>`, `BuildError`, `validate_coverage`, and the unit tests.
- `crates/adapter/net/http/api/src/lib.rs` — **modify.** `pub mod rate;` + re-exports + module-doc bullet.
- `CHANGELOG.md` — **modify.** `[Unreleased] → Added`.

No `Cargo.toml` change — no new dependency. Each task is one commit; the tasks together are one PR/issue.

---

## Setup: issue + worktree

- [ ] **Create the issue and the isolated worktree**

```bash
gh issue create \
  --title "feat(net): RateKey + RateLimitConfig + boot-time coverage (Slice 0, PR 4)" \
  --label enhancement \
  --body "Slice 0 PR 4 of the net-http construction surface (spec: docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md, plan: docs/superpowers/plans/2026-07-04-net-http-rate-coverage.md), closing Slice 0:

- \`RateKey\` trait (finite universe via \`all()\`) + \`LimitPolicy\` + \`LimitDecl\` + total \`RateLimitConfig<K>\`
- \`BuildError\` + \`validate_coverage\`: a config missing a \`K\` variant, or with a bad policy param, is a boot failure (ADR-0034 §3)
- No new dependency; no runtime; validator is unit-tested standalone for Slice 2's \`stack()\`/\`build()\` to call"
```

Note the issue number `#<N>` for the PR body.

```bash
git worktree add .claude/worktrees/net-http-rate-coverage -b feat/net-http-rate-coverage main
cd .claude/worktrees/net-http-rate-coverage
```

All subsequent tasks run inside this worktree.

---

## Task 4.1: `RateKey` + `LimitPolicy` + `LimitDecl` + `RateLimitConfig<K>` — the vocabulary

**Files:**
- Create: `crates/adapter/net/http/api/src/rate.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: `std::collections::HashMap`, `std::hash::Hash`.
- Produces:
  - `oath_adapter_net_http_api::RateKey` — `trait RateKey: Hash + Eq + Clone + Send + Sync + 'static { fn all() -> &'static [Self] where Self: Sized; }`.
  - `oath_adapter_net_http_api::LimitPolicy` — `#[non_exhaustive] enum { TokenBucket { rate: u32, burst: u32 }, Concurrency { max: u32 } }` (`Copy`).
  - `oath_adapter_net_http_api::LimitDecl` — `#[non_exhaustive] enum { Policy(LimitPolicy), GlobalOnly }` (`Copy`).
  - `oath_adapter_net_http_api::RateLimitConfig<K>` — `struct { pub global: LimitPolicy, pub local: HashMap<K, LimitDecl> }`.
  - Task 4.2 adds `BuildError` + `validate_coverage` to this module and consumes exactly these types.

- [ ] **Step 1: Write the failing test**

Create `crates/adapter/net/http/api/src/rate.rs` with the module doc and only the tests, and add `pub mod rate;` + `pub use rate::{LimitDecl, LimitPolicy, RateKey, RateLimitConfig};` + the module-doc bullet (Step 3) to `lib.rs`:

```rust
//! Boot-time pacing coverage: the `RateKey` universe, the `LimitPolicy`/
//! `LimitDecl` classification vocabulary, the total `RateLimitConfig<K>` map,
//! and the `validate_coverage` construction-time check (ADR-0034 §3).
//!
//! A `RateLimitConfig<K>` is **total**: every `K::all()` variant must be
//! explicitly classified — `LimitDecl::Policy` or `LimitDecl::GlobalOnly`,
//! never "absent". A missing or ill-configured bucket is caught at
//! construction ([`validate_coverage`]), so it is a boot failure rather than a
//! first-live-order 429 → 15-minute IBKR penalty box. This module is pure data
//! + one validator; the `RateLimit` layer that consumes it lands in Slice 1.

#[cfg(test)]
mod tests {
    use super::{LimitDecl, LimitPolicy, RateKey, RateLimitConfig};
    use std::collections::HashMap;

    /// A stand-in endpoint key for the tests — the shape an adapter provides.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum TestKey {
        PlaceOrder,
        Snapshot,
        History,
    }

    impl RateKey for TestKey {
        fn all() -> &'static [Self] {
            &[Self::PlaceOrder, Self::Snapshot, Self::History]
        }
    }

    #[test]
    fn rate_key_all_is_drift_proof() {
        // Exhaustive `match` with no wildcard arm: adding a `TestKey` variant
        // fails to compile HERE, forcing whoever adds it to also list it in
        // `all()`; the length assertion catches a variant added to the enum
        // but dropped from `all()`.
        fn is_listed(k: TestKey) -> bool {
            match k {
                TestKey::PlaceOrder | TestKey::Snapshot | TestKey::History => true,
            }
        }
        assert!(TestKey::all().iter().copied().all(is_listed));
        assert_eq!(TestKey::all().len(), 3);
    }

    #[test]
    fn config_classifies_every_key_explicitly() {
        let cfg = RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 10, burst: 20 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
                (TestKey::Snapshot, LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, burst: 5 })),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(cfg.local.len(), 3);
        assert_eq!(cfg.global, LimitPolicy::TokenBucket { rate: 10, burst: 20 });
        assert_eq!(cfg.local[&TestKey::History], LimitDecl::GlobalOnly);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type RateKey` / `LimitPolicy` / `LimitDecl` / `RateLimitConfig`.

- [ ] **Step 3: Implement the vocabulary**

Insert between the module doc and the tests in `rate.rs`:

```rust
use std::collections::HashMap;
use std::hash::Hash;

/// An adapter's rate-limit key with a **finite universe** — the enumeration
/// that makes the boot-time coverage check possible (ADR-0034 §3).
///
/// `Clone` is doubly-earned: `http::Extensions::insert` demands it, and `Retry`
/// clones the request per attempt (Slice 1), so a stamped key survives replay.
/// The universe is kept generic (not erased to `u32`/`&str`) precisely so
/// [`validate_coverage`] can iterate every variant.
pub trait RateKey: Hash + Eq + Clone + Send + Sync + 'static {
    /// Every key in the universe. Its exhaustiveness is what the coverage check
    /// trusts; an adapter keeps it drift-proof (`strum::VariantArray` or an
    /// exhaustive-`match` test), keeping this trait dependency-free.
    fn all() -> &'static [Self]
    where
        Self: Sized;
}

/// A single pacing policy applied to one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitPolicy {
    /// A refilling token bucket: `rate` tokens/second, up to `burst` in hand.
    TokenBucket {
        /// Steady-state tokens per second (must be `>= 1`).
        rate: u32,
        /// Maximum tokens available at once (must be `>= 1`).
        burst: u32,
    },
    /// A concurrency cap: at most `max` in-flight requests in this scope.
    Concurrency {
        /// Maximum concurrent requests (must be `>= 1`).
        max: u32,
    },
}

/// How one endpoint is paced — an **explicit** classification. There is no
/// "absent" arm: totality (every [`RateKey`] variant classified) is what the
/// boot check enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitDecl {
    /// This endpoint has its own local policy (in addition to the global one).
    Policy(LimitPolicy),
    /// This endpoint is paced by the global policy only — declared on purpose.
    GlobalOnly,
}

/// A **total** pacing configuration: a required `global` policy plus a
/// per-endpoint classification for every key in the [`RateKey`] universe.
///
/// [`validate_coverage`] rejects a `local` map that is not total over
/// `K::all()`, so forgetting to pace a new endpoint is a boot failure.
#[derive(Debug, Clone)]
pub struct RateLimitConfig<K> {
    /// The account-wide policy every request is subject to.
    pub global: LimitPolicy,
    /// The per-endpoint classification. Must be total over `K::all()`.
    pub local: HashMap<K, LimitDecl>,
}
```

In `lib.rs`, add the module and re-export (keep the existing alphabetical `pub mod` / `pub use` ordering — `rate` sits between `error` and `service`), and extend the module-doc list:

```rust
//! - [`rate`] — `RateKey`, the `LimitPolicy`/`LimitDecl` vocabulary, the total
//!   `RateLimitConfig`, and the boot-time `validate_coverage` check
```

```rust
pub mod rate;
```

```rust
pub use rate::{LimitDecl, LimitPolicy, RateKey, RateLimitConfig};
```

(Task 4.2 extends this re-export to `pub use rate::{BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_coverage};`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api rate && just lint`
Expected: PASS, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): RateKey + LimitPolicy/LimitDecl + total RateLimitConfig"
```

---

## Task 4.2: `BuildError` + `validate_coverage` — the boot-time check

**Files:**
- Modify: `crates/adapter/net/http/api/src/rate.rs`, `crates/adapter/net/http/api/src/lib.rs`

**Interfaces:**
- Consumes: Task 4.1's `RateKey`, `LimitPolicy`, `LimitDecl`, `RateLimitConfig`; `std::fmt`.
- Produces:
  - `oath_adapter_net_http_api::BuildError` — `#[non_exhaustive] enum { UndeclaredKey(String), InvalidPolicy(String) }` (`Debug`, `thiserror::Error`, `PartialEq`, `Eq`).
  - `oath_adapter_net_http_api::validate_coverage` — `pub fn validate_coverage<K: RateKey + fmt::Debug>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>`. Slice 2's `stack()`/`build()` call it before assembling layers.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `rate.rs` (extend the `use super::…` line to `use super::{BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_coverage};`):

```rust
    /// A total, param-sane config over `TestKey` — the baseline the negative
    /// tests mutate.
    fn total_config() -> RateLimitConfig<TestKey> {
        RateLimitConfig {
            global: LimitPolicy::TokenBucket { rate: 10, burst: 20 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::Policy(LimitPolicy::Concurrency { max: 1 })),
                (TestKey::Snapshot, LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, burst: 5 })),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        }
    }

    #[test]
    fn total_config_validates() {
        assert_eq!(validate_coverage(&total_config()), Ok(()));
    }

    #[test]
    fn missing_key_is_undeclared() {
        let mut cfg = total_config();
        cfg.local.remove(&TestKey::History);
        let err = validate_coverage(&cfg).unwrap_err();
        assert!(matches!(err, BuildError::UndeclaredKey(ref k) if k.contains("History")));
    }

    #[test]
    fn zero_rate_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 0, burst: 5 }),
        );
        assert!(matches!(validate_coverage(&cfg), Err(BuildError::InvalidPolicy(_))));
    }

    #[test]
    fn zero_burst_token_bucket_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::Snapshot,
            LimitDecl::Policy(LimitPolicy::TokenBucket { rate: 5, burst: 0 }),
        );
        assert!(matches!(validate_coverage(&cfg), Err(BuildError::InvalidPolicy(_))));
    }

    #[test]
    fn zero_concurrency_max_is_invalid() {
        let mut cfg = total_config();
        cfg.local.insert(
            TestKey::PlaceOrder,
            LimitDecl::Policy(LimitPolicy::Concurrency { max: 0 }),
        );
        assert!(matches!(validate_coverage(&cfg), Err(BuildError::InvalidPolicy(_))));
    }

    #[test]
    fn bad_global_policy_is_invalid() {
        let mut cfg = total_config();
        cfg.global = LimitPolicy::TokenBucket { rate: 0, burst: 1 };
        assert!(matches!(validate_coverage(&cfg), Err(BuildError::InvalidPolicy(_))));
    }

    #[test]
    fn global_only_endpoints_need_no_local_params() {
        // A `GlobalOnly` decl carries no policy, so it is always coverage-valid
        // (it is paced by the already-validated global).
        let cfg = RateLimitConfig {
            global: LimitPolicy::Concurrency { max: 2 },
            local: HashMap::from([
                (TestKey::PlaceOrder, LimitDecl::GlobalOnly),
                (TestKey::Snapshot, LimitDecl::GlobalOnly),
                (TestKey::History, LimitDecl::GlobalOnly),
            ]),
        };
        assert_eq!(validate_coverage(&cfg), Ok(()));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `just check`
Expected: FAIL — `cannot find type BuildError` / `cannot find function validate_coverage`.

- [ ] **Step 3: Implement `BuildError`, `LimitPolicy::validate`, and `validate_coverage`**

Add `use std::fmt;` to the imports at the top of `rate.rs`. Add the `validate` method in an `impl LimitPolicy` block below the enum, and append `BuildError` + `validate_coverage` after `RateLimitConfig`:

```rust
impl LimitPolicy {
    /// Reject non-sensical policy parameters (ADR-0034 §3 / spec: `rate == 0`,
    /// `burst == 0`, `max == 0`).
    fn validate(self) -> Result<(), BuildError> {
        match self {
            Self::TokenBucket { rate, burst } => {
                if rate == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket rate must be >= 1, got {rate}"
                    )));
                }
                if burst == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "token-bucket burst must be >= 1, got {burst}"
                    )));
                }
                Ok(())
            }
            Self::Concurrency { max } => {
                if max == 0 {
                    return Err(BuildError::InvalidPolicy(format!(
                        "concurrency max must be >= 1, got {max}"
                    )));
                }
                Ok(())
            }
        }
    }
}

/// A construction-time pacing-config failure — the boot-time guard that turns a
/// missing or nonsensical bucket into a startup error instead of a live 429
/// (ADR-0034 §3). Non-generic: the offending key is rendered to a `String` so
/// `stack()`/`build()` can return `Result<_, BuildError>` regardless of `K`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// A [`RateKey`] variant is not classified in `local` — the map is not
    /// total over `K::all()`.
    #[error("rate-limit key `{0}` is not classified in the config (every RateKey::all() variant must be declared)")]
    UndeclaredKey(String),
    /// A policy carries out-of-range parameters (`rate`/`burst`/`max` of 0).
    #[error("invalid rate-limit policy: {0}")]
    InvalidPolicy(String),
}

/// Validate that `cfg` is a **total**, param-sane pacing configuration: the
/// `global` policy is valid, and every [`RateKey`] variant is classified with a
/// valid policy (ADR-0034 §3). Slice 2's `stack()`/`build()` call this before
/// assembling the stack, so a coverage gap is a boot failure.
///
/// # Errors
/// [`BuildError::UndeclaredKey`] if a `K::all()` variant is absent from
/// `cfg.local`; [`BuildError::InvalidPolicy`] if the global or any local policy
/// has an out-of-range parameter.
pub fn validate_coverage<K>(cfg: &RateLimitConfig<K>) -> Result<(), BuildError>
where
    K: RateKey + fmt::Debug,
{
    cfg.global.validate()?;
    for key in K::all() {
        match cfg.local.get(key) {
            None => return Err(BuildError::UndeclaredKey(format!("{key:?}"))),
            Some(LimitDecl::Policy(policy)) => policy.validate()?,
            Some(LimitDecl::GlobalOnly) => {}
        }
    }
    Ok(())
}
```

Extend the `lib.rs` re-export to `pub use rate::{BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_coverage};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api rate && just lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/rate.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): BuildError + validate_coverage — boot-time pacing coverage"
```

---

## Task 4.3: CHANGELOG, full gate, PR

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added` (after the PR 3 construction-seams entry):

```markdown
- `oath-adapter-net-http-api` boot-time pacing coverage — the `RateKey` trait
  (finite universe via `all()`), the `LimitPolicy`/`LimitDecl` classification
  vocabulary, the total `RateLimitConfig<K>` map, `BuildError`, and the
  standalone `validate_coverage` check: an unclassified endpoint or an
  out-of-range policy param is a boot failure, not a first-live-order 429
  (ADR-0034 §3). Closes Slice 0 of the net-http construction surface.
```

- [ ] **Step 2: Full local gate**

Run: `just ci`
Expected: green (fmt, lint, test + doctests, doc, deny, typos, machete — no new dep, so `deny`/`machete` are unaffected).

- [ ] **Step 3: Commit, push, PR**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): net-http boot-time pacing coverage (Slice 0 PR 4)"
git push -u origin feat/net-http-rate-coverage
gh pr create \
  --title "feat(net): RateKey + RateLimitConfig + boot-time coverage (Slice 0 PR 4)" \
  --body "Closes #<N>

Slice 0 **PR 4** of the net-http construction surface (spec: docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md) — **closes Slice 0**.

- **\`RateKey\`** — the adapter's endpoint key with a finite universe (\`fn all() -> &'static [Self]\`), kept generic so the coverage check can iterate every variant.
- **\`LimitPolicy\`** (\`TokenBucket { rate, burst }\` / \`Concurrency { max }\`) + **\`LimitDecl\`** (\`Policy\` / \`GlobalOnly\`) — explicit per-endpoint classification; there is no \"absent\" arm.
- **\`RateLimitConfig<K>\`** — a total map (required \`global\` + \`local\` over \`K::all()\`).
- **\`BuildError\`** + **\`validate_coverage\`** — the pure construction-time check: an unclassified key → \`UndeclaredKey\`; an out-of-range \`rate\`/\`burst\`/\`max\` → \`InvalidPolicy\`. A missing bucket is a boot failure, not a 15-minute IBKR penalty box (ADR-0034 §3).

No new dependency; no runtime; no layer. Slice 2's \`stack()\`/\`build()\` will call \`validate_coverage\` before assembling the stack.

Next: **Slice 1** — the resilience layers (\`Retry\`, \`RateLimit\` (consumes this config + constructs \`Guarded\`), \`CircuitBreaker\`, \`Tracing\`), needing \`MockTimer\`-driven timing tests and the per-request \`Scope\`/\`RateLimit<K>\` extension.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (PR 4 roadmap, foundation plan lines 435–441 + spec lines 298–363):**
- `RateKey` trait, exact spec signature (`Hash + Eq + Clone + Send + Sync + 'static`, `fn all() -> &'static [Self] where Self: Sized`) — Task 4.1. ✅
- `LimitPolicy { TokenBucket { rate, burst }, Concurrency { max } }` — Task 4.1. ✅
- `LimitDecl { Policy(LimitPolicy), GlobalOnly }` — Task 4.1. ✅
- `RateLimitConfig<K> { global: LimitPolicy, local: HashMap<K, LimitDecl> }` — Task 4.1. ✅
- `BuildError` (`thiserror`; `UndeclaredKey`, bad-policy-params) — Task 4.2. "missing-global" intentionally omitted (Decision 2: `global` is a required field, structurally unrepresentable). ✅
- `validate_coverage`: `local` total over `K::all()`, `global` present, param sanity (`rate/burst/max >= 1`) → `Result<(), BuildError>` — Task 4.2. ✅
- Tests: missing `K` → `Err(UndeclaredKey)`; total → `Ok`; bad params (local + global) → `Err`; test `RateKey` with an exhaustive-`match` drift guard — Tasks 4.1/4.2. ✅
- Deferred (correctly absent): the `RateLimit<K>` layer, per-request `Scope`/`RateLimit<K>` extension + call-site fail-closed (spec lines 342–357), `HttpConfig`, `stack()`/`build()` — Slice 1/Slice 2 (Decision 1). ✅

**Placeholder scan:** none — every code step carries the actual code, every run step the actual command.

**Type consistency:** `RateKey::all()` signature identical in the trait def (Task 4.1) and the `TestKey` impl (Task 4.1 tests); `LimitPolicy`/`LimitDecl` variant names identical across the definitions, `total_config()` (Task 4.2), and every test; `RateLimitConfig` field names (`global`, `local`) consistent between the struct, both tasks' tests, and `validate_coverage`; `validate_coverage<K: RateKey + fmt::Debug>(&RateLimitConfig<K>) -> Result<(), BuildError>` matches the `Interfaces` block and all call sites; `BuildError::{UndeclaredKey, InvalidPolicy}` consistent between the enum, `LimitPolicy::validate`, `validate_coverage`, and the tests; `lib.rs` re-exports accumulate to `pub use rate::{BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_coverage};`.

**Known risks to watch during impl:**
- Clippy nursery `missing_const_for_fn` may ask `LimitPolicy::validate` to be `const` — it can't (`format!` + `String`), so no action; if clippy asks for `const` on a genuinely-const fn elsewhere, add it.
- `#[non_exhaustive]` + `#[derive(PartialEq, Eq)]` on `BuildError` is fine within-crate (tests `matches!`/`assert_eq!` on it); external exhaustive matching is intentionally blocked.
- `format!("{key:?}")` requires `K: fmt::Debug` — bounded on `validate_coverage` only (Decision 3), so the `RateKey` trait stays exactly as the spec defines it.
- `RateLimitConfig<K>` derives `Debug` conditionally; the workspace `missing_debug_implementations` lint is satisfied by the derive without forcing `Debug` onto `RateKey`.
