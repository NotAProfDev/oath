# Numeric types: fixed-point `i128` on the wire, two-domain exact/analytical split

`Price` and `Quantity` (ADR-0002's canonical model) are **fixed-point scaled
integers**, not a decimal type. `rust_decimal` is dropped from `oath-model`: it
carries a validity invariant (the `flags` word bounds the scale and reserves bits),
so not every 16-byte pattern is a valid `Decimal` — which makes `bytemuck::Pod`
unsound and breaks "read slot bytes → deterministic value" (ADR-0002 / 0020). A
scaled integer is trivially POD (every bit pattern valid), deterministic, and
branch-free. This mirrors NautilusTrader, which carries no decimal type on the wire.

## The numbers — `Price`, `Quantity`, money ops

- **Width: always `i128`/`u128`** (not a `cfg`-switched i64/i128 like Nautilus's
  `high-precision` flag). The cost of always-128 is concentrated almost entirely in
  **full-depth order books** (≈2× the bytes on the one hottest message family);
  trades/quotes barely move (≤1 cache line), and i128 multiply/divide is slower but
  lives on the low-rate order/fill/PnL path, not the data plane. We take the
  simplification — one format forever, no 64→128 migration, crypto/wei handled from
  day one — and treat **depth-message layout as a separately-optimizable concern**
  (packed/delta form later if it ever bottlenecks; the delta-fold note in
  [bus-backend-realization](../design/bus-backend-realization.md) already anticipates
  it). The depth tail does not wag the whole-model dog.
- **Signedness: `Price` is signed `i128`, `Quantity` is unsigned `u128`.** Negative
  prices are real (WTI −$37, calendar/inter-commodity spreads, basis instruments).
  `Quantity` is a **magnitude**; **direction lives in `Side`** (the single source of
  truth, per the glossary) and **signed exposure is derived in `Position`**, never
  stored as a signed quantity — this keeps "a Quantity is a magnitude" a type
  invariant, matches how venues report (size + side), and buys a bit of range where
  magnitudes get huge (wei).
- **Precision lives with the instrument, not on the wire.** The wire/Event-Log
  `Price`/`Quantity` are **precision-free raw `i128`/`u128` newtypes** (clean 16
  bytes, no padding — self-describing `{raw, precision}` is rejected for the wire
  because `i128`'s 16-byte alignment pads `{i128, u8}` to 32 bytes *and* reintroduces
  the padding-determinism hazard). Precision/tick/lot are **instrument metadata**
  (sourced from symbology / the contract — IBKR's `minTick` etc.), resolved once at
  the boundary. This is sound because a price **always travels under an
  instrument-keyed topic** (`instance-key` carries the symbol, ADR-0020), so the
  scale is never orphaned. A **self-describing in-process working type is optional
  ergonomics**, not part of the canonical model contract — adopt it lazily where
  threading the instrument gets painful; the MVP ships with raw newtypes +
  instrument-taking conversions.
- **Money-op contract — no bare arithmetic.** `Price`/`Quantity` do not expose
  `+ - * /`. The exact-domain ops are a small set of **explicit, checked** functions.
  Add/sub use `checked_*` and **error rather than wrap** (a silent wrap in accounting
  is the unrecoverable money bug; Rust release builds wrap on bare `*` by default).
  `notional = price × qty` overflows `i128` at wei scale, so products **widen to a
  256-bit intermediate, rescale (÷10ᵏ), then checked-narrow to `i128`** — using a
  **vetted pure-Rust, no-`unsafe` bigint** (e.g. `bnum`), not hand-rolled money math
  (same principle as buying redb's crash-testing). It is confined to a few cold-path
  functions, so the software-256 cost is irrelevant.
- **Rounding — two contexts.** Internal accounting (avg fill price, P&L allocation,
  commission) uses **round-half-to-even (banker's)** as the one documented default
  (no accumulating bias). The **order-emission boundary** (price→tick, qty→lot)
  rounds **explicitly and direction-aware per call** (a buy limit rounds *down*; qty
  rounds *down* to not exceed lot/budget) — never an implicit global mode.

## Two numeric domains

- **Exact domain — fixed-point `i128`.** Prices, quantities, money, P&L, order
  fields, the bus, the Event-Log. Exact, deterministic, POD, zero-copy.
- **Analytical domain — `f64`.** Strategy indicators, signals, statistical features.
  Indicators (EMA, stddev, log-returns) involve `exp`/`log`/`sqrt` and
  non-terminating ratios that **have no exact fixed-point representation** — `f64` is
  the correct type for that work, not a compromise. Strategies **convert at the
  boundary**: fixed→float on input (one convert + multiply, negligible vs the
  indicator math), float→fixed on order emission — and that reverse conversion **is**
  the tick/lot rounding point, so it is not wasted work. Exact position/P&L
  *accounting* stays in the exact domain (the Core kernel, ADR-0008); strategies read
  exact business state for decisions and do their *analytics* in float.

## Determinism scope (refines ADR-0012)

ADR-0012's "a recorded run replays bit-exactly" carries an implicit qualifier that
`f64` makes explicit: bit-exact replay re-runs the **same machine code over the same
logged inputs** → identical floats only on the **same binary + same target**. The
guarantee is therefore **layered**:

- **Exact domain** (ordering, matching, accounting, P&L) — **bit-exact,
  cross-platform, audit-grade** (integers + integer time + single-writer, ADR-0005 /
  0006 / 0008).
- **Strategy float domain** — **bit-exact replay on same binary + same target**;
  **cross-target = parity within bounded tolerance** (drift surfaced, not bit-exact).
  This is achievable because Rust is **strict IEEE-754 by default** (no implicit FMA
  contraction / fast-math), `+ − × ÷` and `sqrt` are correctly-rounded across IEEE
  platforms, the **only** cross-target divergence is transcendental `libm`
  implementations, and the strategy fold is **single-threaded by construction**
  (ADR-0012 / 0013) so no nondeterministic parallel reduction exists.
- **NaN/inf policy:** indicators must be NaN/inf-safe (or the framework guards) —
  `NaN != NaN` silently breaks replay-equality.
- **Reserved lever** for future cross-platform float bit-exactness: replace the
  platform `libm` with a single portable correctly-rounded math library. Deferred,
  not MVP.

## Real-money correctness

Layout does not give correctness — no layout makes "wrong precision applied"
impossible (even self-describing `{raw, precision}` is wrong if constructed wrong).
Fixed-point already eliminates the worst class (silent float-rounding drift); the
residual risk is a discrete 10ᵏ scale error, which is wildly out-of-band and trips
risk/sanity bounds immediately. The correctness stack:

1. Precision is applied **only** through typed functions that *require* the
   instrument — misapplication won't compile.
2. **Round-trip property tests / fuzz:** `from_decimal(to_decimal(p, i), i) == p`.
3. **No bare arithmetic; errors never wrap** (above).
4. **ADR-0006 reconciliation is the money-moving backstop:** the order adapter
   re-derives human price/qty from raw + contract tick/lot, validates, the broker
   echoes the accepted order, and reconciliation compares broker-truth to intent — a
   scale bug cannot reach a fill silently.

## Considered options

- _`rust_decimal` inner type_ — rejected: not byte-castable (validity invariant →
  `Pod` unsound), 16 bytes, software arithmetic on the hot path. Kept only at the
  adapter boundary (parsing) and frontend (display), never on the bus or log.
- _`i64` / 9-dp default with an `i128` `cfg` feature flag_ (NautilusTrader) —
  rejected: `i64` cannot hold wei (10 ETH = 1e19 > `i64::MAX`), and `cfg`-switched
  primitive widths split the on-disk format and add complexity. Chosen: always-128,
  depth optimized separately.
- _Self-describing `{raw, precision}` on the wire_ — rejected: 32-byte padded
  footprint + padding-determinism hazard. Chosen: raw-only wire, precision from
  instrument metadata; self-describing is an optional in-process ergonomic.
- _`{whole, frac}` split / rational `{num, den}`_ — rejected: the split still needs a
  precision to interpret `frac` (it just moves the problem) and wrecks arithmetic;
  rational denominators grow unbounded and need gcd-normalization. Neither buys
  correctness over single fixed-point.
- _Signed `Quantity`_ — rejected: duplicates `Side` as a second source of truth for
  direction. Chosen: unsigned magnitude + `Side` + derived `Position` exposure.

## Consequences

- **Refines ADR-0002** (numeric inner type) and **ADR-0012** (float-determinism
  scope); `rust_decimal` leaves `oath-model`'s dependencies.
- **Couples to symbology** (the next parked thread): precision, tick size, and lot
  size are exactly the instrument metadata the symbology layer must supply.
- **Adds a `bnum`-class pure-Rust bigint dependency** for the 256-bit money
  intermediate (cold path only).
- The exact/analytical boundary is the same "convert at the edge" pattern already
  adopted for event ingestion — float→fixed on order emission is where tick/lot
  rounding lives.
