# Type placement: shared contracts sink to `oath-model`, behavior stays in `*/api`

[ADR-0009](0009-crate-topology-spine-inverted-process-aligned.md) made `oath-model`
the spine-inverted root and the `<subsystem>/api` crates the trait-definers, but it
fixed *crate* topology, not where individual **types** live. That left a real
ambiguity — surfaced by `Signal`, whose [README table](../../README.md) row placed it
in `oath-strategy-api` while the dependency graph shows **Core does not depend on
`strategy-api`**, so a `Signal` Core ingests off the Bus could not live there without
inverting the graph. This ADR states the placement rule and applies it.

## Decision

### The rule

`oath-model` holds every type that **crosses a process boundary** (a wire / Event-Log
payload) **or is a shared canonical record referenced by ≥2 subsystems**. The
`<subsystem>/api` crates hold **behavior (traits)** and **single-process-private
types**. Shared *contracts* sink to the root; *behavior* and *privates* stay in the
process crates. This is ADR-0009's spine inversion applied at type granularity.

### Placements

| Type | Home | Rationale |
|---|---|---|
| `Signal` | **`oath-model`** | Canonical Bus message (Strategy Node → Core). Core does not depend on `strategy-api`, so the payload must be at the shared root. `strategy-api` keeps the `Strategy` **trait** plus signal-construction ergonomics — never the payload type. |
| `Instrument` | **`oath-model`** | Shared reference-data record (ADR-0025) read by adapters, Core money-math, and the frontend. The **record** is model; **resolution** is an `adapter-api` trait; **storage** is a `persistence-api` repository trait. |
| `Position` | **`oath-model`** | The canonical, observable record: a Business-State payload (ADR-0014) pushed on the Bus and read by the frontend *and* by strategies (ADR-0023, "strategies read exact business state"). One canonical `Position` for MVP. |
| `Account`, `Source` | **`oath-model`** | Cross-boundary primitives — `Position` keys and order routing. |
| `Decision` | **`core-api`** | Glossary: "internal to Core, never on the Bus" — a single-process type, deliberately **not** in model. The rule cuts both ways. |
| Core lot-level accounting state | **`core-portfolio`** | The machinery behind `Position` (running cost basis, per-fill lots). Private to one process. |

### The `Position` / `PositionView` judgment call

A separate Core-internal accounting type (richer than the observable record) is
promoted **only if and when** the lot-level machinery needs fields observers must not
see. MVP ships **one canonical `Position`** in `oath-model`; the split is deferred,
not pre-built — illegal-states-unrepresentable does not require a second type until
there is private state to hide.

## Consequences

- **README correction:** the table's "`oath-strategy-api`: … Signal types" becomes
  "`Strategy` trait + Signal *ergonomics*; the canonical `Signal` payload lives in
  `oath-model`." The dependency graph (`strathost → model`) already supports this;
  only the prose changed.
- Gives a **mechanical test** for every future type: *does it cross a process boundary
  or get read by two subsystems?* → `oath-model`; else → the owning `*/api` or process
  crate. Removes per-type debate during implementation.
- Confirms `oath-model` carries data + payloads only; **no traits** live there (traits
  are behavior → `*/api`), preserving ADR-0009's "api = traits" split.

## Relationships

Refines **ADR-0009** (spine-inverted topology, now at type granularity). Rests on the
boundary facts of **ADR-0013** (`Signal` is the strategy→Core message), **ADR-0014**
(`Position` is observable Business State), **ADR-0025** (`Instrument` reference
record), and the glossary's "`Decision` is Core-internal." Glossary terms unchanged.
