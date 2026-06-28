# `oath-model` numeric primitives (slice 1) — design

**Status:** Approved design, pre-implementation.
**Date:** 2026-06-28.
**Crate:** `oath-model` (`crates/model`).

## Context

`oath-model` is the root contract of the workspace (ADR-0009): every other crate
depends inward on it, and nothing it exposes may depend on anything else. Today the
crate is a two-line skeleton (`//!` doc + `#![forbid(unsafe_code)]`). Nothing above
it — `*/api` trait crates, the Kernel, Policies, adapters — can carry a real
signature until the root primitives exist. This slice gives the crate its first real
content: the **exact-domain numeric primitives** (`Price`, `Quantity`, `Side`) and
their error type.

This is deliberately the smallest defensible root slice: one issue, one PR. It does
**not** attempt the full `oath-model` contract (symbology, time, the zero-copy
layout discipline) — each of those is a later slice with its own design and ADR
backing.

### Governing ADRs

- **ADR-0023** — numeric types: fixed-point `i128`, two-domain exact/analytical
  split. The direct source for `Price`/`Quantity` shape and the money-op contract.
- **ADR-0002 / ADR-0020** — backend-agnostic Bus, one canonical message model; the
  universal type bound is `Serialize`, POD is a backend-specific discipline.
- **ADR-0027** — wire representation: layered bounds, the `zerocopy` layout crate,
  `u64`-nanos `Timestamp`, deferred schema versioning. Governs the *deferred* work
  below; reconciles the `oath-model` manifest. *(Landed on `main` in #49.)*
- **CONTEXT.md** — the ubiquitous-language glossary (`Price`, `Quantity`, `Side`,
  `Position`, `Source`, `InstrumentId`).

## Goal

Implement `Price`, `Quantity`, `Side`, and `ArithmeticError` as value types with
checked exact-domain arithmetic, total ordering where it makes sense, `serde`
support, and property-tested invariants — and reconcile the crate manifest by
dropping `rust_decimal`/`uuid`/`time`.

## Scope (in)

- `Price(i128)` — signed; raw constructor/accessor; checked add/sub; ordering;
  `serde`.
- `Quantity(u128)` — unsigned magnitude; same surface; checked sub underflows to an
  error.
- `Side { Buy, Sell }` — direction enum with `opposite()`; `serde`.
- `ArithmeticError { Overflow, Underflow }` — `thiserror`.
- Unit, `proptest` property, and `serde` round-trip tests for every operation and
  invariant.
- Manifest reconciliation: remove `rust_decimal`, `uuid`, `time`, and the
  `cargo-machete` ignore block; add `proptest` and `serde_json` as dev-dependencies.

## Non-goals (deferred — each a later issue/PR)

| Deferred item | Why deferred | Lands with |
| --- | --- | --- |
| `zerocopy` layout (`FromBytes`/`IntoBytes`/`Immutable`/`KnownLayout`), `#[repr(C)]`/`transparent`, compile-time `size_of`/no-padding asserts | No consumer yet — Bus/Event-Log unbuilt. ADR-0027 §3 names **`zerocopy`** (not bytemuck): its derive rejects padding at compile time and handles `i128` 16-byte alignment. Isolated `oath-model` dep. | The zero-copy layout slice |
| Instrument-keyed `from_decimal`/`to_decimal` | Precision is instrument metadata (ADR-0023); the `Instrument` type does not exist yet | The symbology slice (ADR-0025) |
| `notional = price × qty` (256-bit widen → rescale → checked-narrow) | Needs a `bnum`-class bigint (ADR-0023/0027); cold path, no consumer | The notional/money-ops slice |
| Rounding (banker's; tick/lot at order emission) | Needs instrument tick/lot metadata | The symbology / execution slices |
| `Timestamp` | ADR-0027 §4 fixes it as `#[repr(transparent)]` **`u64` nanoseconds since the Unix epoch, UTC** (UnixNanos); signed durations are a separate `i64`-nanos type. No `time` crate on the wire | The time slice |
| `InstrumentId` / `Symbol` / `Source` / `Instrument` | ADR-0025/0026: deterministic `(Env, generation, counter)` identity, **not** UUID | The symbology slice |
| `Position` signed-exposure derivation | Exposure is *derived* from `Quantity` + `Side`, never stored signed (ADR-0023) | The portfolio slice |

## Architecture

### Module structure (one module per primitive)

```text
crates/model/src/
  lib.rs        // crate docs, `mod` declarations, `pub use` re-exports
  error.rs      // ArithmeticError
  price.rs      // Price + #[cfg(test)] mod tests
  quantity.rs   // Quantity + #[cfg(test)] mod tests
  side.rs       // Side + #[cfg(test)] mod tests
```

Each primitive is an independently understandable, independently testable unit with
one responsibility. `Side` lives next to the numerics it constrains. The deferred
work has obvious homes: `from_decimal`/`to_decimal` in `price.rs`/`quantity.rs`,
`notional` later, `zerocopy` derives on each type. `lib.rs` re-exports the public
types so consumers write `oath_model::Price`, not `oath_model::price::Price`.

### Types & invariants

**`Price`** — signed fixed-point, raw `i128`. Negative prices are real (WTI −$37,
calendar/inter-commodity spreads, basis instruments), so the inner type is signed.

```rust
pub struct Price(i128);

impl Price {
    pub const fn from_raw(raw: i128) -> Self;
    pub const fn raw(self) -> i128;
    pub const fn checked_add(self, rhs: Price) -> Result<Price, ArithmeticError>;
    pub const fn checked_sub(self, rhs: Price) -> Result<Price, ArithmeticError>;
}
```

Derives: `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize,
Deserialize`. Ordering is the natural integer order (needed for book/limit
comparisons later). **Ordering invariant (documented on the type):** because a
`Price` is a *precision-free* raw scale (ADR-0023), comparison is meaningful **only**
among prices of the same instrument/precision — `price_aapl < price_btc` compiles but
means nothing across instruments. `Ord` is retained (same-topic book/limit logic
needs it; ADR-0023 accepts this); the constraint is stated in the doc-comment where a
reader hits it, mirroring the `Quantity`-is-a-magnitude invariant.

**`Quantity`** — unsigned **magnitude**, raw `u128`.

```rust
pub struct Quantity(u128);

impl Quantity {
    pub const fn from_raw(raw: u128) -> Self;
    pub const fn raw(self) -> u128;
    pub const fn checked_add(self, rhs: Quantity) -> Result<Quantity, ArithmeticError>;
    pub const fn checked_sub(self, rhs: Quantity) -> Result<Quantity, ArithmeticError>;
}
```

Same derives as `Price`. **Invariant (documented on the type):** a `Quantity` is a
magnitude; direction lives in `Side` (the single source of truth); signed exposure
is *derived* in `Position` (a later slice), never stored as a signed quantity. The
unsigned inner type makes a negative quantity unrepresentable by construction, and
buys range where magnitudes get huge (wei).

**`Side`** — the single source of truth for direction.

```rust
pub enum Side { Buy, Sell }

impl Side {
    pub const fn opposite(self) -> Side;
}
```

Derives: `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`. No
`Ord` — there is no natural ordering of `Buy`/`Sell`.

**`ArithmeticError`** — `thiserror` enum.

```rust
pub enum ArithmeticError {
    /// Result exceeds the maximum representable value.
    Overflow,
    /// Result is below the minimum representable value
    /// (e.g. subtracting a larger magnitude from a smaller `Quantity`).
    Underflow,
}
```

No layout attributes (`repr`, `zerocopy` derives) on any type — honoring the
"no wire-layout commitment in this slice" boundary. The types are nonetheless
chosen to be POD-eligible later (`i128`/`u128`/fieldless enum) so the deferred
`zerocopy` slice is purely additive.

## Serialization stance

Settled by ADR-0002/0020 and restated by ADR-0027 §1–2:

- **`serde` is the universal contract.** Every canonical type derives
  `Serialize`/`Deserialize`, enforced by a `serde_json` round-trip test (see
  Testing). `oath-model` defines **no** custom serialization trait; the per-backend
  variation lives in the `Bus` trait (`oath-bus-api`), not in the data.
- **Zero-copy is a backend-specific discipline, layered later.** The shared-memory /
  read-in-place family (iceoryx2, Aeron IPC, mmap'd Event-Log replay) will add a
  `zerocopy` bound on its own impls; stream backends (Kafka, RabbitMQ,
  Aeron-over-network, TCP) need only `Serialize`. Deferring the layout work here is
  *consistent* with that layering, not a shortcut.
- **Recorded gotcha for the zero-copy slice:** a fieldless `Side` enum is not
  `zerocopy::FromBytes` (enums have invalid bit patterns), so it will need
  `#[repr(u8)]` + `TryFromBytes` (or a `u8` wire newtype) when that slice lands.

## Error handling

All fallible arithmetic returns `Result<_, ArithmeticError>`. No bare `+ - * /`
operators are exposed on `Price`/`Quantity` (ADR-0023: a silent wrap in accounting
is the unrecoverable money bug). Internals delegate to the standard
`i128::checked_add` / `u128::checked_sub` etc. and map the `None` case:

- `Quantity::checked_add` → `Overflow` (past `u128::MAX`).
- `Quantity::checked_sub` → `Underflow` (when `rhs > self`).
- `Price::checked_add` / `checked_sub` → `Overflow` for the positive bound,
  `Underflow` for the negative bound, determined from the operand signs (signed
  overflow only occurs in a direction the operand signs reveal).

No `unwrap`/`expect`/indexing in non-test code (workspace lints are warn-level and
CI is warning-free). `panic_in_result_fn` stays satisfied because these functions
return `Result` and never panic.

`checked_add`/`checked_sub` are `const fn`: the standard checked ops are const-stable
(≥ Rust 1.47, well under MSRV 1.90) and the sign-mapping is an explicit `match` (no
`?`, whose `Try` desugaring is not const). This is purely additive — `from_raw`/`raw`
are already `const` — and lets the arithmetic compose into `const` contexts (e.g.
table-driven tick/lot constants in a later slice).

## Testing

Unit tests per operation (in each file's `#[cfg(test)] mod tests`, which is
lint-exempt for `unwrap`):

- add/sub happy paths for `Price` and `Quantity`.
- `Price::checked_add` at `i128::MAX` → `Err(Overflow)`; at `i128::MIN`
  (`Price::from_raw(i128::MIN).checked_add(Price::from_raw(-1))`) → `Err(Underflow)`.
- `Price::checked_sub(Price::from_raw(0), Price::from_raw(i128::MIN))` →
  `Err(Overflow)` (subtracting `i128::MIN` exceeds the positive bound);
  `Price::from_raw(i128::MIN).checked_sub(Price::from_raw(1))` → `Err(Underflow)`.
  These `i128::MIN` lines exercise the sign-mapping branch most likely to survive a
  mutant.
- `Quantity::checked_add` at `u128::MAX` → `Err(Overflow)`.
- `Quantity::checked_sub` with `rhs > self` → `Err(Underflow)`.
- `Side::opposite` maps `Buy↔Sell`; ordering of `Price` matches integer order.
- **serde round-trip:** a `proptest` round-trip through `serde_json`
  (`from_str(&to_string(x)) == x`) over the full `i128`/`u128` range for
  `Price`/`Quantity`, plus an exact-shape unit check for `Side` (serializes to
  `"Buy"`/`"Sell"`) — enforcing the `Serialize`/`Deserialize` universal-contract
  claim (ADR-0002/0020) at the root. (`serde_test::assert_tokens` was evaluated and
  rejected: `serde_test` 1.x has no `I128`/`U128` tokens, so it cannot represent the
  numeric newtypes; `serde_json` was verified to round-trip the full range
  losslessly, including `i128::MIN`/`MAX` and `u128::MAX`.)

Property tests via `proptest` (new dev-dependency):

- **Raw round-trip:** `Price::from_raw(x).raw() == x` for all `x: i128`;
  `Quantity::from_raw(x).raw() == x` for all `x: u128`.
- **Add/sub inverse:** for in-range `a, b`, `a.checked_add(b)?.checked_sub(b)? == a`
  (both types).
- **Add commutativity:** `Price::checked_add` — `a.checked_add(b)` equals
  `b.checked_add(a)` (both `Ok`-equal or both `Err`).
- **`Quantity` sub boundary:** `a.checked_sub(b)` is `Err(Underflow)` iff `b > a`,
  else `Ok(a - b)`.
- **`Side::opposite` involution:** `s.opposite().opposite() == s`.

## Dependencies

The current manifest carries deps "ahead of first use" via a `cargo-machete`
ignore list. This slice reconciles it (partial ADR-0027 reconciliation — `zerocopy`
and `bnum` are intentionally left for their deferred slices).

**Before** (`crates/model/Cargo.toml`):

```toml
[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
rust_decimal = { version = "1", features = ["serde-with-str"] }
uuid = { version = "1", features = ["v4", "serde"] }
time = { version = "0.3", features = ["serde"] }

[package.metadata.cargo-machete]
ignored = ["rust_decimal", "serde", "thiserror", "time", "uuid"]
```

**After:**

```toml
[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
```

- `rust_decimal` — removed (ADR-0023: it leaves `oath-model`; not byte-castable).
- `uuid` — removed permanently (ADR-0026/0027: ids are deterministic
  `(Env, generation, counter)`, not UUID).
- `time` — removed from `oath-model`; it relocates to the **adapter boundary**
  (parsing venue timestamp strings) and **frontend** (display) per ADR-0027 §4.
  The wire `Timestamp` (later slice) is a raw `u64`, needing no `time` crate.
- `serde` / `thiserror` — now actually used (derives + error type), so they leave
  the `cargo-machete` ignore list, which becomes empty and is deleted.
- `proptest` — added to `[workspace.dependencies]` (workspace pattern) and consumed
  as a dev-dependency.
- `serde_json` — added likewise (dev-only); the `proptest` round-trip deserializes
  what it serialized over the full numeric range (P1 from review). `serde_test` was
  rejected: it has no `i128`/`u128` tokens.

## Definition of done

- `just ci` is green — fmt, fmt-toml, typos, lint (clippy `all` = deny), check, test
  (+ doctests), deny, doc, machete, gitleaks, actionlint, shellcheck.
- `just mutants-diff` shows **zero surviving mutants** on the changed files (local
  check; cargo-mutants is intentionally **not** part of CI).
- All public items documented (`missing_docs`), including enum variants; `Debug`
  derived everywhere (`missing_debug_implementations`).
- No `unsafe`, no `unwrap`/`expect`/indexing in non-test code.
- Delivered as one issue → one worktree branch (`feat/oath-model-numeric-primitives`)
  → one PR that `Closes` the issue.

## Future slices (roadmap, not this PR)

1. **zerocopy layout** — `zerocopy` derives, `#[repr(C)]`/`transparent`,
   compile-time `size_of`/no-padding assertions, `Side` via `TryFromBytes`
   (ADR-0027). Completes the manifest reconciliation (`zerocopy` dep).
2. **Timestamp** — `u64` UnixNanos + `i64`-nanos duration type (ADR-0027 §4).
3. **Symbology** — `InstrumentId`/`Symbol`/`Source`/`Instrument`, and the
   instrument-keyed `from_decimal`/`to_decimal` conversions (ADR-0025).
4. **notional / money ops** — 256-bit widen via `bnum`, banker's rounding
   (ADR-0023).
5. Upward: `oath-model` message types, then the `*/api` trait crates.
