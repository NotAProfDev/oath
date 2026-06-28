# Wire representation: zero-copy layout, `u64` nanos time, fixed-size `InstrumentId`, deferred schema versioning

[ADR-0002](0002-backend-agnostic-bus-canonical-message-model.md) parked two
concerns it could not yet resolve: how `#[repr(C)]` payloads stay sound under
zero-copy, and how they evolve under schema change. [ADR-0023](0023-numeric-types-fixed-point-exact-analytical-split.md)
and [ADR-0025](0025-symbology-instrument-id-reference-data.md) then made a string
of *POD-enabling* choices (fixed-point `i128` not `Decimal`, fixed-size
self-identifying `InstrumentId` not `String`) without ever stating the layout
contract they were enabling. This ADR states it: the universal bound on a canonical
message, the **backend-specific** zero-copy bound layered on top, the crate that
expresses it, the wire form of `Timestamp`, and the **deferred-but-reserved** schema
versioning scheme. MVP scope is single-host, one supervised build per Environment.

## Decision

### 1. Layered bounds — `Serialize` is universal, zero-copy is backend-specific

A canonical message's **only universal bound is serialize-ability**
(`Serialize`/`Deserialize`). In-place byte-castability is an **additional** bound
that **one backend family requires**, not a property of the model. This is the
ADR-0020 refinement of ADR-0002 made concrete:

- `oath-bus-api`'s trait is generic over `M: Serialize` — every backend can move
  every message.
- Shared-memory backends add `where M: <zero-copy bound>` **on their own impl**.
- A message designed with heap content (a future cold-path `Vec` field) carries only
  `Serialize` and is simply **not eligible** for the shared-memory family. The model
  stays honest rather than degraded.

### 2. Two backend families, not "iceoryx vs everything"

Zero-copy splits two ways, and only one side imposes a layout bound:

| Family | Backends | Bound it needs |
|---|---|---|
| **Shared-memory, read-in-place** | iceoryx2, Aeron **IPC**, an mmap'd **Event-Log replay** | stable byte layout (POD-ish) |
| **Stream / serialize** | Kafka, RabbitMQ, Aeron-over-**network**, TCP | `Serialize` / `Deserialize` only |

Aeron IPC is shared-memory (mapped log buffers) and is paired with SBE precisely
because in-place decode wants a fixed layout — the **same** need as iceoryx2, not the
Kafka need. Kafka's "zero-copy" is the broker's `sendfile` disk→socket, not a typed
consumer view. **Event-Log replay is in the in-place family too**: the layout bound
is therefore not an iceoryx quirk — it also makes mmap'd log replay fast, which is
why it is a model-level *capability* and not buried in one backend.

### 3. The layout crate — `zerocopy`, because `unsafe` is denied

`unsafe_code = "deny"` (workspace-wide) means hand-rolled `transmute` is impossible;
the unsafe **must** be encapsulated in a vetted crate. We pick **`zerocopy`**
(`FromBytes`, `IntoBytes`, `Immutable`, `KnownLayout`, `Unaligned`) over `bytemuck`
for two OATH-specific reasons:

- Its derive **rejects padding at compile time**. Padding bytes are uninitialized and
  nondeterministic; exposing them would poison the bit-exact replay ADR-0023 / 0005
  depend on. The compiler now enforces the no-padding invariant 0023 only *asserted*.
- **`i128` is 16-byte aligned** (Rust ≥ 1.77). `zerocopy`'s alignment-aware `Ref` and
  `Unaligned` read `i128`-bearing structs from packed log records and ring offsets
  more gracefully than `bytemuck`'s align-or-panic casts.

The derive must live on the model type (orphan rule), so **`zerocopy` is an isolated
`oath-model` dependency**, following the same "isolated, swappable backend dep"
discipline already used for the time/decimal deps in `crates/model/Cargo.toml`.

### 4. `Timestamp` is `u64` nanoseconds since the Unix epoch, UTC

The wire/Event-Log `Timestamp` is a `#[repr(transparent)]` **`u64` nanoseconds since
the Unix epoch**, UTC, never a calendar/`OffsetDateTime` type (not POD; reintroduces
the validity-invariant hazard that removed `rust_decimal`). This is the "integer
time" ADR-0023's determinism scope already presumed, and it is NautilusTrader's
`UnixNanos` exactly. The `time` crate **stays only at the adapter boundary** (parsing
venue `UTCTimestamp` strings such as FIX `TransactTime`) and the frontend (display) —
never on the wire or log. **Durations/offsets are a separate `i64`-nanos type**
(signed deltas).

`u64` over `i64`: range to ~2554 and no negative-time values a trade clock never
wants, mirroring the `Quantity`-is-a-magnitude discipline (ADR-0023). Pre-epoch
instants are not a trading concern.

### 5. Schema versioning — deferred pre-1.0, reserved, model-wide bumps

OATH is pre-release; API and message churn are expected, so **no versioning
machinery is built before 1.0** — message layout is simply "whatever the current
build says." But the seam is **reserved** so adopting it later is not a format break:

- The per-boot `IncarnationStarted` marker (ADR-0026) gains a **`schema_version:
  u16`** field, hardcoded `1` for now. It costs one field and stamps the writing
  build's schema onto every record until the next marker — **incarnation-granular
  versioning with zero per-message cost.**
- Versioning is an **Event-Log concern, not a live-Bus concern**: within one
  Environment the Supervisor boots a single build (ADR-0001 / 0018), so live
  processes share one layout and there is no in-flight skew to pay for. The live Bus
  stays unversioned; only the durable log, which outlives the build, needs it.
- The future rule (not built now): **layout is immutable per `(model,
  schema_version)`; a change bumps a single model-wide version and the fold retains
  old decoders and upcasts old→current — it never reinterprets old bytes** (keeps
  replay bit-exact and the audit log "interpretable forever", ADR-0025, without ever
  rewriting it). A *per-type* SBE-style header was rejected for MVP: it reintroduces
  per-record framing and partly defeats the unversioned-live-Bus win.

### 6. `InstrumentId` is a fixed `[u8; 32]` self-identifying name

[ADR-0025 §3](0025-symbology-instrument-id-reference-data.md) chose the wire form
(Choice A: the human-readable self-identifying name itself) but left its length
*"TBD against real IBKR `contractDetails`; 24 bytes is a placeholder."* This fixes it:
**`InstrumentId` is `[u8; 32]`**, a `#[repr(transparent)]` NUL-padded normalized-ASCII
name with the asset class as a textual prefix (`EQ:` / `OPT:` / `FUT:` / `FX:` /
`CR:`), behind a named `INSTRUMENT_ID_LEN` constant.

The length was sized against the real standards, longest-first:

| Anchor | Len | Prefixed wire form | Bytes |
|---|---|---|---|
| **OCC option symbol** (binding) | 21 | `OPT:AAPL240119C00150000` | **25** |
| PermID (instrument, numeric) | ≤~18 | `EQ:300000000000000123` | ~21 |
| Futures (venue-qualified) | ~18 | `FUT:ES.GLOBEX.20241220` | ~22 |
| FIGI / ISIN | 12 | `EQ:BBG000B9XRY4` / `EQ:US0378331005` | 15 |
| FX pair | 6 | `FX:EURUSD` | 9 |

The **OCC 21-char option symbol** (the second asset class on the roadmap) is the
binding constraint at 25 prefixed bytes — so the placeholder **24 is provably too
small**, and 32 holds every standard anchor with margin. FIGI (fixed 12) and PermID
(numeric, ≤~18) both sit well inside. 32 is a power-of-two, friendly as a topic key
and log record; the bytes are a wire/log/key cost only, since ADR-0025 §3 lets a
process intern the name to a local integer for hot-path comparisons.

- **Canonical encoding:** uppercase alphanumerics + `: - .`, **NUL-padded only**
  (never spaces), validated at construction — so two adapters encoding the same anchor
  produce **byte-identical** ids (ADR-0025 §4 "agree by construction" becomes a type
  invariant) and equality is a plain `[u8; 32]` compare.
- **Anchor priority** is ISIN/FIGI/OCC (open standards) first; **PermID is a
  single-vendor (LSEG) fallback**, not a co-equal anchor — the precise ladder is left
  to ADR-0025's deferred curation/mapping work.
- **Outliers use the reserved escape hatch, not a wider array:** anything past 32 —
  realistically an on-chain/DEX token by its 42-char `0x…` address — is the trigger to
  adopt the logged-`u64` surrogate ADR-0025 §3 reserved for the dense-sharding future,
  not a reason to widen the array and tax every equity key.

Pre-1.0 the constant is still revisable against a concrete contract that exceeds it;
32 is the considered default, not an unknown carried into the type.

## Considered options

- *Per-message version header (SBE-style)* — rejected for MVP: correct for
  mixed-version wires, but a single supervised build per Environment has no in-flight
  skew, so it is per-message cost for a problem the topology does not have. The log
  gets versioning from the incarnation marker instead.
- *`bytemuck`* — viable and simpler, but weaker padding enforcement and align-or-panic
  casts; the determinism + `i128`-alignment arguments favour `zerocopy`. The choice is
  contained to derives + backend cast-sites, so it is reversible.
- *`core::mem::TransmuteFrom` (Project Safe Transmute)* — the eventual *blessed*
  mechanism (compiler-proved transmutation, no third-party crate). **Reserved as a
  future implementation**: it is nightly-only and unusable at MSRV 1.90. Revisit once
  it stabilizes; it could retire the `zerocopy` dependency.
- *`OffsetDateTime`/calendar type on the wire* — rejected: not POD, padded, and
  carries a validity invariant — the same reasons `rust_decimal` was dropped.
- *`i64` nanos instant* — rejected: admits negative times a trade clock never wants
  and caps earlier (~2262); `u64` is the magnitude-clean choice and matches Nautilus.
- *`InstrumentId` of 24 bytes (ADR-0025 placeholder)* — rejected: cannot hold a
  max-root OCC option symbol (25 prefixed bytes). *48 bytes* — rejected for MVP: taxes
  every equity key for a long-venue-qualified-crypto case that is far off and already
  covered by the reserved `u64` surrogate. *Structured `{ class: u8, anchor: [u8; K] }`*
  — rejected: loses the "Event Log interpretable with no external table" payoff that
  motivated Choice A; the flat ASCII name keeps logs human-readable.

## Consequences

- **Refines ADR-0002 / 0020** (states the layered bound + names the crate) and
  **ADR-0023** (fixes `Timestamp` as `u64` UnixNanos, completing the integer-time
  scope) and **extends ADR-0026** (`IncarnationStarted` gains `schema_version`).
- **`oath-model` manifest is now stale and must be reconciled** when implementation
  starts: drop `rust_decimal` (ADR-0023) and `uuid` (ADR-0026 rejected UUID ids for
  deterministic `(Env, generation, counter)`), add `zerocopy` and the `bnum`-class
  bigint (ADR-0023); `time` is retained but demoted to a boundary/display dep.
- Hot-path messages must be authored to the zero-copy discipline (fixed-size,
  `#[repr(C)]`/`transparent`, no padding, no heap) and carry the `zerocopy` derives;
  compile-time `size_of`/no-padding assertions enforce it.
- The live Bus carries no version overhead; the Event Log is self-describing at
  incarnation granularity. Building the decoder-retention/upcasting machinery is
  deferred to the 1.0 schema-evolution effort.

## Relationships

Refines **ADR-0002** (canonical message model), **ADR-0020** (`Serialize`-universal /
POD-backend-specific bound), **ADR-0023** (integer-time + POD-enabling numerics).
Extends **ADR-0026** (`IncarnationStarted` marker) and **discharges ADR-0025**'s
parked `InstrumentId`-length TBD (`[u8; 32]`). Rests on **ADR-0001 / 0018**
(one supervised build per Environment ⇒ unversioned live Bus) and **ADR-0005**
(deterministic fold ⇒ replay must never reinterpret old bytes). Glossary: `Timestamp`
in [CONTEXT.md](../../CONTEXT.md) (already implementation-free; unchanged).
