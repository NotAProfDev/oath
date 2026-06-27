# Symbology: self-identifying `InstrumentId`, off-wire `Instrument` record, deterministic mapping

ADR-0023 deferred precision, tick, lot, and multiplier to "instrument metadata the
symbology layer must supply." This ADR is that layer. It defines how OATH names a
tradable instrument, where its reference data lives, how a broker's native id maps to
the canonical name, and how all of it stays replay-safe (ADR-0005) and POD on the
wire (ADR-0020 / 0023). MVP is **IBKR-first, Equity-only**; the harder asset classes
are designed-for but not built.

## Decision

### 1. Identity — `InstrumentId` (self-identifying), `Symbol` demoted to ticker

The canonical identity is **`InstrumentId`**: venue-**independent** where a
venue-independent identity exists (so the same instrument at two brokers collapses to
one id, and positions/risk aggregate — ADR-0024), **venue-qualified** where it does
not, but **always normalized**, never a raw broker string. It is **self-identifying**
— a stable, standards-based (ISIN/FIGI/OCC) or deterministically-derived denotation
that needs **no OATH-private id registry** to say *which* instrument it is, and never
drifts. This is distinct from *self-explaining*: it pins identity, not attributes.

`Symbol` is **demoted** to the human venue **ticker** ("AAPL", "ESM4") — a display
attribute on the `Instrument` record, never the identity (a ticker is reused across
companies and differs across venues). `Source` (the producing adapter) remains a
market-data **routing** coordinate, never part of identity: the same `InstrumentId`
from two Sources is two streams.

### 2. Reference data — the `Instrument` record, off-wire, typed by class

The **`Instrument`** record is resolved reference data keyed **`(InstrumentId,
Source)`** (tick/lot are venue facts, so one `InstrumentId` may resolve to two
`Instrument`s, parallel to two streams). It is the **single home** for the ADR-0023
precision and **never travels on the wire or Event Log**. It is a **shared core**
that every class has and the money math needs day one — `InstrumentId`, `Source`,
asset-class, quote currency, **tick**, **lot/min size**, **multiplier** — plus a
**per-asset-class typed tail** (expiry/strike/right/underlying), **not** a flat struct
of optionals (illegal states stay unrepresentable). MVP implements only the `Equity`
variant.

### 3. Wire form — the self-identifying name itself (Choice A)

The `InstrumentId` travels on the Bus and Event Log as its **fixed-size
self-identifying name** (e.g. `EQ:US0378331005`), not an opaque surrogate. The Event
Log is therefore interpretable forever with no external table. Attributes stay
off-wire in the `Instrument` record. A process **may** intern the name to a local
integer for hot-path lookups **provided that integer never crosses the wire**. The
fixed length is **TBD against the real IBKR `contractDetails`** (24 bytes is a
placeholder; option/FIGI keys may need more). A logged-assignment wire `u64` is
**reserved** for the dense-sharding future (ADR-0021 bucket) and is **not** MVP.

### 4. Mapping & agreement — deterministic rule + curated overrides; never guess

Each adapter (the anti-corruption layer) derives `InstrumentId` from the broker's
reference data via a **shared, versioned normalization ruleset**, so two adapters
**agree by construction** when both report the same anchor. A small **operator-curated
override table** handles no-anchor and known-conflict cases. The OpenFIGI *lookup
service* is a **deferred** fallback.

**Safety invariant (day one):** **no external anchor ⇒ venue-qualified id ⇒ no
collapse, ever, until curated.** A *missed* collapse is a degraded view; a *false*
collapse silently merges positions — a money bug. **Never guess a collapse.**

A **cross-`Source` price-plausibility monitor** is a backstop against gross
bad-collapse: per `InstrumentId`, prices across its Sources should agree within a
**band over a sustained window**, **compared in a common currency** (read from the
`Instrument` record). On breach it **alerts and quarantines** the suspect mapping —
**out-of-fold** (Telemetry/alert plane, ADR-0014), never a silent trade or instant
halt. It is a **broad net** (catches order-of-magnitude mismatches, not same-price
collisions) and complements — does not replace — curation and the ADR-0006
reconciliation backstop. (Distinct from the *in-fold* price-sanity risk guard that
stops Core acting on an implausible quote, which ADR-0023 anticipates.)

### 5. Resolution — a logged Core input, split by determinism need

Bringing an instrument into an Environment is a **logged Core input** ("instrument
registered"), before the first message referencing it. Metadata splits two ways:

- **Fold-relevant** (multiplier, and anything the canonical fold reads) → **logged**,
  so Replay reproduces P&L bit-exactly (ADR-0005).
- **Boundary/display-only** (the `Symbol` ticker, adapter-side rounding tick, display
  precision) → cached in the `Instrument` record, **never logged**.

Timing: a **config-declared universe resolved at Environment start**, plus
**logged on-demand additions** for dynamic universes (option chains, scans). Every
addition — boot or on-demand — is the same logged registration; **no implicit lazy
resolution** (mirrors ADR-0013's strategy-admission handshake).

### 6. Lifecycle — immutable id, logged change

- **Ticker change** (`FB → META`) is a **non-event for identity** (the id anchors on
  ISIN): just refresh the `Symbol` field off-log. *This is the payoff of the
  ISIN anchor.*
- **Identity-changing events** (ISIN change on re-domicile/merger): `InstrumentId` is
  **immutable**; mint a **new** id plus a **logged succession link** (old → new).
- **Metadata changes** (tick regime, contract spec): the `Instrument` record is
  **time-versioned**; the fold-relevant subset changes only via a **logged "instrument
  updated"** input; boundary fields refresh off-log.
- **Position-moving corporate actions** (splits, dividends): **logged Core inputs**,
  translated by the adapter from the venue notice, applied deterministically by the
  fold.
- **Futures rollover is position lifecycle, not identity:** each contract is its own
  `InstrumentId` (expiry in the id); a "continuous contract" is a *derived*
  strategy-side concept.

MVP scope: equity **splits** + **ticker change** only; mergers/spin-offs/rights,
succession UX, and futures roll are deferred (the seams above are fixed now).

### 7. Derivatives — reference by `InstrumentId`, composition off-wire

Options/futures/spreads reference their **underlying and legs by `InstrumentId`**,
recursively; the composition (underlying, legs+ratios, strike/expiry/right) lives in
the **off-wire `Instrument` typed tail**, so the wire id stays a compact anchor even
for complex instruments. Non-tradable underlyings (an index) get a **reference-only**
`InstrumentId` (no `Account`/`Position` ever keyed by it). This is *metadata about a
self-identifying instrument*, so it does not reintroduce a registry dependency. MVP
builds none of it; the seam is reserved.

## Considered options

- _Opaque `u64` surrogate on the wire_ — rejected: the id↔instrument binding is
  arbitrary and OATH-assigned, so the Event Log's meaning would depend on a mutable
  private registry that, if lost/drifted/collided, makes audit history unreadable or
  silently wrong. Self-identifying names avoid this; local interning recovers the
  speed.
- _Venue-in-identity (`Symbol.Venue`, à la NautilusTrader)_ — rejected: unambiguous
  and always-works, but yields **no** cross-venue collapse, which OATH wants for
  cross-broker positions/risk and cross-asset budget rules (ADR-0024). OATH anchors
  the identity component on an external standard instead.
- _Fat `Symbol` carrying metadata inline_ — rejected: bloats every topic key and log
  record and contradicts ADR-0020 / 0023. Metadata lives off-wire in `Instrument`.
- _Flat `Instrument` struct with optional fields_ — rejected: "valid-by-convention"
  (an equity with a strike set); the per-class typed tail keeps illegal states
  unrepresentable (ADR-0023's "misapplication won't compile" ethos).
- _Central registry / security master now_ — deferred, **not** rejected: production
  grade will want one, but because `InstrumentId` is self-identifying it is an
  **additive** evolution of the resolution/curation seam (it supplies attributes and
  curation, never the identity binding), not a redesign. MVP ships the decentralized
  degenerate form (rule + local overrides).

## Consequences

- **Discharges ADR-0023's coupling:** the `Instrument` record is the single typed home
  for precision/tick/lot/multiplier, taken only through instrument-requiring
  functions.
- **`InstrumentId` is the symbol component of the ADR-0020 topic instance-key**, and
  the key for `Position` `(Account, InstrumentId)` and `Signal`s.
- **Resolution and lifecycle become Event-Log inputs** (ADR-0005): instrument
  registration, metadata updates, succession links, and corporate actions are all
  logged, deterministic, replayable; the off-bus instrument registry is recovery /
  bootstrap substrate.
- **Cross-broker collapse over `InstrumentId` (ADR-0024)** is sound because the id is
  venue-independent where it can be and the no-anchor fallback never guesses.
- **ADR-0006 reconciliation remains the money-moving backstop**; the price-plausibility
  monitor is a cheap out-of-fold complement.
- **New glossary terms:** `InstrumentId`, `Instrument`; `Symbol` redefined; `Account`,
  `Position` (added with ADR-0024).
- **Production central security master** is the anticipated evolution (parked).
- **Parked sub-questions:** fixed-size id length vs real IBKR contracts; **combo
  identity** (structural-encode vs leg-decompose); the central security-master
  service; OpenFIGI fallback; the corporate-action taxonomy + succession UX; and the
  durable `Instrument` store as part of the repository-backend decision (ADR-0009 log↔
  repository split — pure-Rust, not Postgres).
