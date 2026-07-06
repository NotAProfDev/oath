# Observability: three planes split across Core's deterministic boundary

Every observer (the Frontend first) sees the running hub through **three distinct
planes**, drawn along Core's deterministic boundary. **Business State** —
positions, P&L, exposure — is the continuous observable subset of Core's
canonical fold, pushed as a single coalesced, Event-Log-seq-stamped snapshot.
**Domain Events** — order placed/filled, signal admitted/rejected, breach
fired/cleared, cancelled-by-risk, alert — are discrete facts the fold produces,
emitted on one durable, must-deliver, ordered narrative stream; each surfaces the
outcome of an internal **Decision** (ADR-0008) as a _derived_ fact, so the
Decision itself stays internal (never on the Bus) while the observable fact still
gets out. **Telemetry** — per-topic throughput, signal-/order-generation rates,
latencies, queue depths, process health — is wall-clock instrumentation _outside_
the fold, self-reported per process (Core, each Adapter, each Strategy Node;
Supervisor adds health), never seq-stamped. The read path is **push-spine**
(observers subscribe) with a **narrow query** escape hatch; observers render all
three planes directly and **never re-fold**.

## Considered options

- _Pull / query as the primary read path_ — rejected: it puts a query-serving
  obligation on Core's single-writer hot path for the steady-state dashboard.
  Push keeps observers off the hot path (Core already emits its outputs); query
  stays a rare, deliberately-narrow escape hatch.
- _Reuse raw operational topics for discrete events_ (the observer stitches its
  own timeline from Core→broker orders, inbound fills on the Event Log, and the
  trapped reject/breach Decisions) — rejected: the sources differ in direction
  and semantics, and Decisions never hit the Bus at all, so every Frontend would
  reverse-engineer an internal timeline. That is not a public contract; it is a
  standing invitation to drift, and it breaks the "depends only on the public
  message model" boundary (CONTEXT: Frontend) the moment a second observer exists.
- _One read-model projector from day one_ — deferred: re-folding canonical state
  in a second process is the two-sources-of-truth trap, unjustified at MVP. The
  durable Domain-Event stream is precisely the substrate such a projector tails
  later, at no new emission cost.
- _One "telemetry" notion for all of it_ — rejected: it crosses the deterministic
  boundary. Business State and Domain Events are products of the fold (replayable,
  seq-stamped / ordered); Telemetry is wall-clock instrumentation that must never
  contaminate canonical state.

## Consequences

- **Core gains an explicit emission obligation:** project Business State and emit
  Domain Events as derived facts of its fold — a public contract, versioned with
  the canonical message model.
- **Two authoritative records, two halves.** The Event Log is _what Core saw_
  (inputs); the Domain-Event stream is _what Core did_ (narrative). Together they
  are the forensic / audit substrate.
- **Telemetry is per-process and out-of-fold**, so Core is never made omniscient
  about processes it cannot see; throughput / health come from the processes
  themselves (Supervisor aggregates health). Telemetry loss is acceptable;
  Business State coalesces (latest wins); Domain Events must not be lost.
- The mechanism that delivers Business State, Domain Events, and query responses
  **without touching the writer thread** is ADR-0015.

## Amendment (2026-07-06): net adapter Telemetry emission

The network adapter (`net-http`) emits its numeric **Telemetry** — circuit-breaker
phase transitions, local pacing (`Throttled`) rejections, retry attempts and
backoff, and pacing permit-wait — through the runtime-neutral
[`metrics`](https://docs.rs/metrics) facade, not `tracing` spans alone. This keeps
the contract crate `oath-adapter-net-api` runtime-neutral (ADR-0029): the crate only
*emits*; the **downstream process binary installs the recorder/exporter**
(Prometheus/OTel/…), and with no recorder installed every emit is a cheap no-op.
Consistent with the plane split above — this Telemetry is wall-clock, per-process,
out-of-fold, and loss-acceptable; it never crosses the deterministic boundary or
touches canonical state. Label cardinality is bounded: the only per-request label is
an adapter-stamped `RouteTemplate` (e.g. `/iserver/account/{id}/order/{id}`), never
the raw ID-bearing path.
