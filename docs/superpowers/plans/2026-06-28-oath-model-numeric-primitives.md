# oath-model Numeric Primitives Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `oath-model` its first real content — the exact-domain numeric primitives `Price`, `Quantity`, `Side`, and their `ArithmeticError`.

**Architecture:** One module per primitive (`error.rs`, `price.rs`, `quantity.rs`, `side.rs`) re-exported from `lib.rs`. Newtypes over raw `i128`/`u128`; no bare arithmetic operators — only checked, `const fn` add/sub that error rather than wrap. `serde` is the universal bound; no zero-copy/POD layout in this slice.

**Tech Stack:** Rust 2024 (MSRV 1.90), `serde` (derive), `thiserror`; dev-only `proptest` + `serde_json`; tooling via `just` (`cargo nextest`, `cargo clippy`, `cargo deny`, `cargo machete`, `cargo mutants`, `taplo`).

**Spec:** [docs/superpowers/specs/2026-06-28-oath-model-numeric-primitives-design.md](../specs/2026-06-28-oath-model-numeric-primitives-design.md)

## Working context

- All work happens in the **existing worktree** `.claude/worktrees/oath-model-numeric-primitives` on branch `feat/oath-model-numeric-primitives` (already created off `main`). Do **not** switch the primary checkout's branch.
- Open a GitHub issue (label `enhancement`) for this slice; the final PR references it with `Closes #N`.
- All code in this plan is **already prototyped and verified** against a faithful copy of the workspace lints, `cargo deny`, tests, doctests, and `rustfmt` — transcribe it verbatim.

## Global Constraints

- **Edition 2024, MSRV 1.90** — no APIs newer than 1.90 (`clippy.toml` pins `msrv = "1.90"`).
- **No `unsafe`** — `lib.rs` keeps `#![forbid(unsafe_code)]`.
- **No bare `+ - * /` on `Price`/`Quantity`** — only checked functions that return `Result` and **error rather than wrap**.
- **`clippy` runs as `-D warnings` with `pedantic` + `nursery` + `cargo` enabled** — so `#[must_use]`, `const fn`, `# Errors` doc sections, and back-ticked doc identifiers are mandatory, not optional. The prototyped code already satisfies this.
- **`missing_docs`** — every public item (including enum variants) carries a doc comment.
- **`unwrap`/`expect`/indexing** are denied in non-test code, but **allowed in tests** (`clippy.toml`: `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-indexing-slicing-in-tests`). The tests here are written without them anyway.
- **`serde` is the universal bound** — every type derives `Serialize`/`Deserialize`. No custom serialization trait, no `repr`/`zerocopy` in this slice.
- **Conventional Commits** (enforced by `commit-msg` hook). The `pre-commit` hook runs `fmt`, `fmt-toml`, `typos`, `lint`, `test-no-run` on every commit; `just doc`, `cargo deny`, `cargo machete`, and the full test run happen at `pre-push`/Task 6.
- **Definition of done:** `just ci` green **and** `just mutants-diff` shows zero surviving mutants; one issue → one PR.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/model/Cargo.toml` | Manifest: drop `rust_decimal`/`uuid`/`time`; add dev-deps `proptest`, `serde_json` |
| `Cargo.toml` (root) | Add `proptest`, `serde_json` to `[workspace.dependencies]` |
| `crates/model/src/lib.rs` | Crate docs, `mod` declarations, `pub use` re-exports |
| `crates/model/src/error.rs` | `ArithmeticError` |
| `crates/model/src/side.rs` | `Side` + tests |
| `crates/model/src/quantity.rs` | `Quantity` + tests |
| `crates/model/src/price.rs` | `Price` + tests |

---

## Task 1: Reconcile the manifest — drop ADR-removed dependencies

**Files:**

- Modify: `crates/model/Cargo.toml`

**Interfaces:**

- Consumes: nothing.
- Produces: a manifest with only `serde` + `thiserror` as dependencies (both still unused, kept in the `cargo-machete` ignore list).

- [ ] **Step 1: Edit `crates/model/Cargo.toml`**

Replace the `[dependencies]` section and the model-specific block so the file reads exactly:

```toml
[package]
name = "oath-model"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }

# cargo-machete: deps declared ahead of first use; prune from `ignored` as they are adopted.
[package.metadata.cargo-machete]
ignored = ["serde", "thiserror"]
```

(This removes `rust_decimal`, `uuid`, `time` and their explanatory comment block, and drops them from the `ignored` list.)

- [ ] **Step 2: Regenerate the lockfile (dependencies were removed)**

Removing dependencies makes `Cargo.lock` stale, and the pre-commit hook runs `cargo check --locked`, which **fails** on a stale lock. Regenerate it:

Run: `cargo check -p oath-model`
Expected: compiles; `git status --porcelain` now shows **both** `crates/model/Cargo.toml` and `Cargo.lock` modified (the orphaned `rust_decimal`/`uuid`/`time` entries and their unique transitive deps are dropped from the lock).

- [ ] **Step 3: Verify machete, locked build, and formatting**

Run: `cargo machete && cargo check -p oath-model --locked && taplo fmt --check`
Expected: `cargo machete` reports no unused dependencies (`serde`/`thiserror` are ignore-listed); `cargo check --locked` now **passes** (the lock is back in sync); no taplo diff.

- [ ] **Step 4: Commit (include `Cargo.lock`)**

```bash
git add crates/model/Cargo.toml Cargo.lock
git commit -m "chore(model): drop rust_decimal, uuid, time deps (ADR-0023/0027)"
```

---

## Task 2: `ArithmeticError`

**Files:**

- Create: `crates/model/src/error.rs`
- Modify: `crates/model/src/lib.rs`
- Modify: `crates/model/Cargo.toml`

**Interfaces:**

- Consumes: nothing.
- Produces: `pub enum ArithmeticError { Overflow, Underflow }` (derives `Debug, Error, Clone, Copy, PartialEq, Eq, Hash`). Used by `Price`/`Quantity` checked operations in Tasks 4–5.

- [ ] **Step 1: Write the failing test + module file**

Create `crates/model/src/error.rs`:

```rust
//! Error type for checked exact-domain arithmetic.

use thiserror::Error;

/// An error from a checked exact-domain arithmetic operation on a `Price` or
/// `Quantity`.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithmeticError {
    /// The result exceeds the maximum representable value.
    #[error("arithmetic overflow: result exceeds the representable maximum")]
    Overflow,
    /// The result is below the minimum representable value (for example,
    /// subtracting a larger magnitude from a smaller `Quantity`).
    #[error("arithmetic underflow: result is below the representable minimum")]
    Underflow,
}

#[cfg(test)]
mod tests {
    use super::ArithmeticError;

    #[test]
    fn variants_are_distinct() {
        assert_ne!(ArithmeticError::Overflow, ArithmeticError::Underflow);
        assert_ne!(
            ArithmeticError::Overflow.to_string(),
            ArithmeticError::Underflow.to_string()
        );
    }
}
```

Replace the entire contents of `crates/model/src/lib.rs` with:

```rust
//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;

pub use error::ArithmeticError;
```

- [ ] **Step 2: Make `thiserror` an active dependency**

In `crates/model/Cargo.toml`, change the `cargo-machete` ignore list to drop `thiserror` (now used):

```toml
[package.metadata.cargo-machete]
ignored = ["serde"]
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p oath-model error::`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Verify lint + format**

Run: `just lint && cargo fmt --all -- --check && taplo fmt --check`
Expected: no warnings, no diffs.

- [ ] **Step 5: Commit**

```bash
git add crates/model/src/error.rs crates/model/src/lib.rs crates/model/Cargo.toml
git commit -m "feat(model): add ArithmeticError"
```

---

## Task 3: `Side` + dev-dependency setup

**Files:**

- Create: `crates/model/src/side.rs`
- Modify: `crates/model/src/lib.rs`
- Modify: `crates/model/Cargo.toml`
- Modify: `Cargo.toml` (root)

**Interfaces:**

- Consumes: nothing.
- Produces: `pub enum Side { Buy, Sell }` with `pub const fn opposite(self) -> Side` (derives `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`).

- [ ] **Step 1: Add `proptest` + `serde_json` to the workspace**

In the root `Cargo.toml`, append these two lines to the end of the `[workspace.dependencies]` block (right after `tracing = "0.1"`):

```toml
proptest = "1"
serde_json = "1"
```

In `crates/model/Cargo.toml`, add a `[dev-dependencies]` section and **delete** the `[package.metadata.cargo-machete]` block entirely (after this task `serde` is used by `Side`, so nothing remains to ignore). The file becomes:

```toml
[package]
name = "oath-model"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: Write the failing test + module file**

Create `crates/model/src/side.rs` with the test module **only** (no `Side` type yet) so the build fails:

```rust
//! Order `Side` — the single source of truth for direction.

#[cfg(test)]
mod tests {
    use super::Side;
    use proptest::prelude::*;

    #[test]
    fn opposite_maps_buy_and_sell() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn serializes_to_expected_shape() {
        assert_eq!(
            serde_json::to_string(&Side::Buy).ok(),
            Some("\"Buy\"".to_owned())
        );
        assert_eq!(
            serde_json::to_string(&Side::Sell).ok(),
            Some("\"Sell\"".to_owned())
        );
    }

    proptest! {
        #[test]
        fn opposite_is_an_involution(s in prop_oneof![Just(Side::Buy), Just(Side::Sell)]) {
            prop_assert_eq!(s.opposite().opposite(), s);
        }
    }
}
```

Update `crates/model/src/lib.rs` to (note alphabetical `mod`/`pub use` ordering, which `rustfmt` enforces):

```rust
//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod side;

pub use error::ArithmeticError;
pub use side::Side;
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p oath-model side::`
Expected: FAIL — compile error `cannot find type \`Side\` in this scope` (and an unresolved `pub use side::Side`).

- [ ] **Step 4: Implement `Side`**

Insert the implementation at the **top** of `crates/model/src/side.rs`, above the `#[cfg(test)]` module (keep the existing `//!` line at the very top):

```rust
use serde::{Deserialize, Serialize};

/// The direction of an order or position: buy or sell.
///
/// `Side` is the single source of truth for direction; `Quantity` stays a
/// magnitude and never encodes sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// A buy: increases a long position.
    Buy,
    /// A sell: increases a short position.
    Sell,
}

impl Side {
    /// Returns the opposite side.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p oath-model side::`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 6: Verify lint, format, and locked build**

Run: `just lint && cargo fmt --all -- --check && taplo fmt --check && cargo check -p oath-model --locked`
Expected: no warnings, no diffs. The `cargo test` run in Step 5 already pulled `proptest`/`serde_json` into `Cargo.lock`, so `cargo check --locked` passes (lock in sync). `git status --porcelain` shows `Cargo.lock` modified — it **must** be committed.

- [ ] **Step 7: Commit (include `Cargo.lock`)**

```bash
git add Cargo.toml Cargo.lock crates/model/Cargo.toml crates/model/src/side.rs crates/model/src/lib.rs
git commit -m "feat(model): add Side direction enum"
```

---

## Task 4: `Quantity`

**Files:**

- Create: `crates/model/src/quantity.rs`
- Modify: `crates/model/src/lib.rs`

**Interfaces:**

- Consumes: `ArithmeticError` (Task 2).
- Produces: `pub struct Quantity(u128)` with `from_raw(u128) -> Quantity`, `raw(self) -> u128` (both `const`), `checked_add(self, Quantity) -> Result<Quantity, ArithmeticError>`, `checked_sub(self, Quantity) -> Result<Quantity, ArithmeticError>` (both `const`; derives `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize`).

- [ ] **Step 1: Write the failing test + module file**

Create `crates/model/src/quantity.rs` with the test module **only**:

```rust
//! Unsigned fixed-point `Quantity`.

#[cfg(test)]
mod tests {
    use super::Quantity;
    use crate::error::ArithmeticError;
    use proptest::prelude::*;

    #[test]
    fn add_and_sub_happy_path() {
        assert_eq!(
            Quantity::from_raw(10).checked_add(Quantity::from_raw(5)),
            Ok(Quantity::from_raw(15))
        );
        assert_eq!(
            Quantity::from_raw(10).checked_sub(Quantity::from_raw(5)),
            Ok(Quantity::from_raw(5))
        );
    }

    #[test]
    fn add_overflow() {
        assert_eq!(
            Quantity::from_raw(u128::MAX).checked_add(Quantity::from_raw(1)),
            Err(ArithmeticError::Overflow)
        );
    }

    #[test]
    fn sub_underflow() {
        assert_eq!(
            Quantity::from_raw(0).checked_sub(Quantity::from_raw(1)),
            Err(ArithmeticError::Underflow)
        );
    }

    proptest! {
        #[test]
        fn raw_round_trip(x in any::<u128>()) {
            prop_assert_eq!(Quantity::from_raw(x).raw(), x);
        }

        #[test]
        fn sub_underflows_iff_rhs_greater(a in any::<u128>(), b in any::<u128>()) {
            let result = Quantity::from_raw(a).checked_sub(Quantity::from_raw(b));
            if b > a {
                prop_assert_eq!(result, Err(ArithmeticError::Underflow));
            } else {
                prop_assert_eq!(result, Ok(Quantity::from_raw(a - b)));
            }
        }

        #[test]
        fn json_round_trip(x in any::<u128>()) {
            let q = Quantity::from_raw(x);
            let back = serde_json::to_string(&q)
                .ok()
                .and_then(|s| serde_json::from_str::<Quantity>(&s).ok());
            prop_assert_eq!(back, Some(q));
        }
    }
}
```

Update `crates/model/src/lib.rs` (insert `quantity` in alphabetical position):

```rust
//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod quantity;
mod side;

pub use error::ArithmeticError;
pub use quantity::Quantity;
pub use side::Side;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p oath-model quantity::`
Expected: FAIL — compile error `cannot find type \`Quantity\``.

- [ ] **Step 3: Implement `Quantity`**

Insert at the top of `crates/model/src/quantity.rs`, below the `//!` line and above the test module:

```rust
use serde::{Deserialize, Serialize};

use crate::error::ArithmeticError;

/// An unsigned, fixed-point quantity carried as a raw `u128` scaled integer.
///
/// A `Quantity` is a **magnitude**: direction lives in [`Side`], and signed
/// exposure is derived in a position, never stored here. The unsigned inner type
/// makes a negative quantity unrepresentable by construction.
///
/// [`Side`]: crate::Side
///
/// ```
/// use oath_model::Quantity;
/// let q = Quantity::from_raw(100);
/// assert_eq!(q.raw(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Quantity(u128);

impl Quantity {
    /// Wraps a raw scaled integer as a `Quantity`.
    #[must_use]
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }

    /// Returns the raw scaled integer.
    #[must_use]
    pub const fn raw(self) -> u128 {
        self.0
    }

    /// Adds two quantities, returning an error on overflow instead of wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] if the result exceeds `u128::MAX`.
    pub const fn checked_add(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Ok(Self(v)),
            None => Err(ArithmeticError::Overflow),
        }
    }

    /// Subtracts one quantity from another.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Underflow`] if `rhs` is greater than `self`
    /// (a quantity is a magnitude and cannot go negative).
    pub const fn checked_sub(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Ok(Self(v)),
            None => Err(ArithmeticError::Underflow),
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p oath-model quantity::`
Expected: `test result: ok. 6 passed` (`add_and_sub_happy_path`, `add_overflow`, `sub_underflow`, `raw_round_trip`, `sub_underflows_iff_rhs_greater`, `json_round_trip`).

- [ ] **Step 5: Verify lint + format**

Run: `just lint && cargo fmt --all -- --check`
Expected: no warnings, no diffs.

- [ ] **Step 6: Commit**

```bash
git add crates/model/src/quantity.rs crates/model/src/lib.rs
git commit -m "feat(model): add Quantity magnitude newtype"
```

---

## Task 5: `Price`

**Files:**

- Create: `crates/model/src/price.rs`
- Modify: `crates/model/src/lib.rs`

**Interfaces:**

- Consumes: `ArithmeticError` (Task 2).
- Produces: `pub struct Price(i128)` with `from_raw(i128) -> Price`, `raw(self) -> i128` (both `const`), `checked_add(self, Price) -> Result<Price, ArithmeticError>`, `checked_sub(self, Price) -> Result<Price, ArithmeticError>` (both `const`; derives `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize`).

- [ ] **Step 1: Write the failing test + module file**

Create `crates/model/src/price.rs` with the test module **only**:

```rust
//! Signed fixed-point `Price`.

#[cfg(test)]
mod tests {
    use super::Price;
    use crate::error::ArithmeticError;
    use proptest::prelude::*;

    #[test]
    fn add_and_sub_happy_path() {
        assert_eq!(
            Price::from_raw(10).checked_add(Price::from_raw(5)),
            Ok(Price::from_raw(15))
        );
        assert_eq!(
            Price::from_raw(10).checked_sub(Price::from_raw(5)),
            Ok(Price::from_raw(5))
        );
    }

    #[test]
    fn add_overflow_and_underflow() {
        assert_eq!(
            Price::from_raw(i128::MAX).checked_add(Price::from_raw(1)),
            Err(ArithmeticError::Overflow)
        );
        assert_eq!(
            Price::from_raw(i128::MIN).checked_add(Price::from_raw(-1)),
            Err(ArithmeticError::Underflow)
        );
    }

    #[test]
    fn sub_min_boundary() {
        assert_eq!(
            Price::from_raw(0).checked_sub(Price::from_raw(i128::MIN)),
            Err(ArithmeticError::Overflow)
        );
        assert_eq!(
            Price::from_raw(i128::MIN).checked_sub(Price::from_raw(1)),
            Err(ArithmeticError::Underflow)
        );
    }

    #[test]
    fn ordering_matches_integers() {
        assert!(Price::from_raw(-1) < Price::from_raw(0));
        assert!(Price::from_raw(0) < Price::from_raw(1));
    }

    proptest! {
        #[test]
        fn raw_round_trip(x in any::<i128>()) {
            prop_assert_eq!(Price::from_raw(x).raw(), x);
        }

        #[test]
        fn add_sub_inverse(a in any::<i128>(), b in any::<i128>()) {
            let pa = Price::from_raw(a);
            let pb = Price::from_raw(b);
            if let Ok(sum) = pa.checked_add(pb) {
                prop_assert_eq!(sum.checked_sub(pb), Ok(pa));
            }
        }

        #[test]
        fn add_commutative(a in any::<i128>(), b in any::<i128>()) {
            prop_assert_eq!(
                Price::from_raw(a).checked_add(Price::from_raw(b)),
                Price::from_raw(b).checked_add(Price::from_raw(a))
            );
        }

        #[test]
        fn json_round_trip(x in any::<i128>()) {
            let p = Price::from_raw(x);
            let back = serde_json::to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Price>(&s).ok());
            prop_assert_eq!(back, Some(p));
        }
    }
}
```

Update `crates/model/src/lib.rs` (insert `price` in alphabetical position — this is the final form):

```rust
//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod price;
mod quantity;
mod side;

pub use error::ArithmeticError;
pub use price::Price;
pub use quantity::Quantity;
pub use side::Side;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p oath-model price::`
Expected: FAIL — compile error `cannot find type \`Price\``.

- [ ] **Step 3: Implement `Price`**

Insert at the top of `crates/model/src/price.rs`, below the `//!` line and above the test module. Note the sign-mapping in the `None` arms — this is what the `i128::MIN` boundary tests guard:

```rust
use serde::{Deserialize, Serialize};

use crate::error::ArithmeticError;

/// A signed, fixed-point price carried as a raw `i128` scaled integer.
///
/// `Price` is precision-free: the scale (tick size) is instrument metadata, not
/// stored here. Negative prices are valid (spreads, basis instruments).
///
/// # Ordering invariant
///
/// Comparison is meaningful **only** among prices of the same instrument and
/// precision. `Ord` is derived because same-instrument book and limit logic needs
/// it, but comparing prices of different instruments compiles and means nothing.
///
/// ```
/// use oath_model::Price;
/// let p = Price::from_raw(12_345);
/// assert_eq!(p.raw(), 12_345);
/// assert_eq!(Price::from_raw(10).checked_add(Price::from_raw(5)), Ok(Price::from_raw(15)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Price(i128);

impl Price {
    /// Wraps a raw scaled integer as a `Price`.
    #[must_use]
    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    /// Returns the raw scaled integer.
    #[must_use]
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// Adds two prices, returning an error on overflow instead of wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] or [`ArithmeticError::Underflow`] if
    /// the result is out of `i128` range.
    pub const fn checked_add(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Ok(Self(v)),
            None if self.0 > 0 => Err(ArithmeticError::Overflow),
            None => Err(ArithmeticError::Underflow),
        }
    }

    /// Subtracts one price from another, returning an error on overflow.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] or [`ArithmeticError::Underflow`] if
    /// the result is out of `i128` range.
    pub const fn checked_sub(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Ok(Self(v)),
            None if rhs.0 < 0 => Err(ArithmeticError::Overflow),
            None => Err(ArithmeticError::Underflow),
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p oath-model price::`
Expected: `test result: ok. 8 passed` (`add_and_sub_happy_path`, `add_overflow_and_underflow`, `sub_min_boundary`, `ordering_matches_integers`, `raw_round_trip`, `add_sub_inverse`, `add_commutative`, `json_round_trip`).

- [ ] **Step 5: Run the doctest**

Run: `cargo test -p oath-model --doc`
Expected: `test result: ok. 2 passed` (the `Price` and `Quantity` doc examples).

- [ ] **Step 6: Verify lint + format**

Run: `just lint && cargo fmt --all -- --check`
Expected: no warnings, no diffs.

- [ ] **Step 7: Commit**

```bash
git add crates/model/src/price.rs crates/model/src/lib.rs
git commit -m "feat(model): add Price fixed-point newtype"
```

---

## Task 6: Full verification + mutation testing + PR

**Files:** none (verification gate). Adds a test only if a mutant survives.

**Interfaces:**

- Consumes: everything from Tasks 1–5.
- Produces: a green `just ci`, zero surviving mutants, and an open PR.

- [ ] **Step 1: Run the full CI gate**

Run: `just ci`
Expected: every stage green — `fmt`, `fmt-toml`, `typos`, `lint`, `check`, `test` (unit + doctests), `deny`, `doc`, `machete`, `gitleaks`, `actionlint`, `shellcheck`. In particular `cargo machete` passes (no ignore list remains) and `just doc` resolves all cross-type intra-doc links now that every primitive exists.

- [ ] **Step 2: Run mutation testing on the diff**

Run: `just mutants-diff`
Expected: `cargo mutants` reports **0 missed / 0 survived** mutants across the changed files. (The checked-op boundary tests, the `i128::MIN`/`MAX` cases, and the `sub_underflows_iff_rhs_greater` property are designed to kill the arithmetic and sign-mapping mutants.)

- [ ] **Step 3: If any mutant survived, add a killing test**

For each surviving mutant cargo-mutants prints (e.g. *"replace `checked_add` match arm `None if self.0 > 0` with `true`"*), add a targeted unit test in the relevant `#[cfg(test)] mod tests` that fails under that mutation, then re-run `just mutants-diff` until zero survive. Commit:

```bash
git add crates/model/src/
git commit -m "test(model): kill surviving arithmetic mutants"
```

(Skip this step's commit if Step 2 already reported zero survivors.)

- [ ] **Step 4: Push and open the PR**

```bash
git push -u origin feat/oath-model-numeric-primitives
gh pr create --fill --label enhancement --body "Implements the oath-model numeric primitives (Price, Quantity, Side, ArithmeticError) per the slice-1 design.

Closes #<ISSUE_NUMBER>"
```

Expected: the `pre-push` hook re-runs `just ci` green; the PR opens; GitHub Actions CI + MSRV job run on the PR.

---

## Self-Review

**Spec coverage:**

- `Price`/`Quantity`/`Side`/`ArithmeticError` → Tasks 5/4/3/2. ✓
- Checked `const fn` add/sub, error-not-wrap, sign-mapping → Tasks 4–5 + boundary tests. ✓
- Ordering invariant documented on `Price` → Task 5 doc comment. ✓
- `Quantity` magnitude invariant documented → Task 4 doc comment. ✓
- `serde` universal bound + round-trip enforcement → derives in Tasks 3–5 + `serde_json` proptest round-trips + `Side` shape check. ✓
- Manifest reconciliation (drop `rust_decimal`/`uuid`/`time`, add `proptest`/`serde_json`, remove machete block) → Tasks 1 + 3. ✓
- Deferred items (zerocopy/POD, decimal conversions, notional, rounding, Timestamp, symbology, Position) → not implemented, by design. ✓
- DoD (`just ci` + `just mutants-diff`) → Task 6. ✓

**Placeholder scan:** No TBD/TODO; every code step contains complete, prototype-verified code; every command has an expected result.

**Type consistency:** `from_raw`/`raw`/`checked_add`/`checked_sub`/`opposite`, `ArithmeticError::{Overflow,Underflow}`, and `Side::{Buy,Sell}` are used identically across the spec, interfaces, and code. `mod`/`pub use` ordering is alphabetical (rustfmt-enforced) and shown in full at each task that touches `lib.rs`.
