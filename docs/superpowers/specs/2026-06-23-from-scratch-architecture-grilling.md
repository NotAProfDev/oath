# From-scratch architecture — grilling summary (2026-06-23)

**Status:** Branch (A) — crate & dependency-graph revision — **closed 2026-06-24**
(ADRs 0007–0009). Branches (B) strategy runtime and (C) Frontend/CLI remain.

## The question

"If we built OATH's scaffolding from scratch, would we do it differently?" Goal:
a high-performance, production-grade, secure, maintainable trading engine with
everything behind traits so backends are swappable.

**Short answer: yes, structurally.** The existing scaffolding assumes a
single-process, statically-composed monolith (`oath-engine` composes all layers
via `EngineBuilder`; dep graph `engine → {everything}`). The design we converged
on is a **single-host, multi-process, event-sourced system communicating over a
swappable message Bus**. The current crate graph is aimed at the monolith we
decided *not* to build — revising it is open branch (A) below.

## Decisions made (see docs/adr/)

- **[ADR-0001](../../adr/0001-single-host-multi-process-topology.md)** —
  Single-host, multi-process topology: one process per Adapter, one Core (risk +
  execution + portfolio, initially also strategies), one or more Strategy Nodes.
  Fault isolation is the driver. Adapters and strategy nodes are hot-pluggable.
- **[ADR-0002](../../adr/0002-backend-agnostic-bus-canonical-message-model.md)** —
  Backend-agnostic Bus behind a trait; **one** canonical message model for all
  backends (`Pod + Serialize`). Per-topic delivery semantics; durability is a
  backend capability (iceoryx2 ephemeral; Kafka / Chronicle Queue durable).
- **[ADR-0003](../../adr/0003-canonical-model-adapter-translation.md)** —
  Canonical core model; each Adapter translates at its boundary (anti-corruption
  layer); central symbology (perm_id / OpenFIGI). `Price`/`Quantity` are
  newtypes over a swappable inner type (`rust_decimal` MVP).
- **[ADR-0004](../../adr/0004-risk-as-continuous-control-loop.md)** — Risk is a
  continuous autonomous control loop with cancel/amend authority, not a pre-trade
  gate. Strategies detect & propose Signals; Core decides & acts.
- **[ADR-0005](../../adr/0005-single-writer-event-sourced-core.md)** —
  Single-writer, event-sourced, deterministic Core. MVP = single-threaded kernel
  + offloaded async I/O (NautilusTrader-validated); disruptor pipeline is a
  later, *measured* optimization. Replay = fold over the Event Log.
- **[ADR-0006](../../adr/0006-broker-reconciliation-contract.md)** — Broker is
  the source of truth. Recovery = replay + reconcile, joined by a client order
  id. Log-before-send ordering invariant. Idempotent submit + queryable order
  state are **mandatory** Broker-adapter capabilities.
- **[ADR-0007](../../adr/0007-binding-time-runtime-pluggable-iff-cross-process.md)** —
  Binding time: runtime-pluggable ⟺ a separate process across the Bus; everything
  in-process is compile-time static. **No `dyn` on any hot path.**
- **[ADR-0008](../../adr/0008-single-owner-kernel-stateless-policies.md)** —
  Single-owner Kernel; risk/execution/portfolio are stateless **Policies** over a
  read-only `StateView`. A Policy emits a **Decision** (actions into a reused
  sink); the Kernel is the sole actor.
- **[ADR-0009](../../adr/0009-crate-topology-spine-inverted-process-aligned.md)** —
  Crate topology: spine-inverted (everything → `oath-model`), process-aligned;
  `<subsystem>/api` = traits, `core/` = the Core process. `oath-engine` and
  `oath-ingest-core` deleted; Event Log / repositories split; new `supervisor` and
  `cli` (Frontend) binaries.

Glossary: **[CONTEXT.md](../../../CONTEXT.md)** (primitives, processes, messages,
persistence/recovery, transport).

## Validated against prior art

NautilusTrader independently uses nearly the same model (single-threaded
deterministic kernel, offloaded I/O, centralized cache, no sharding,
strategy→risk→exec). Key *deliberate* divergence: Nautilus is single-process
with a non-swappable bus; OATH is multi-process with a swappable bus — paying
cross-process complexity to buy crash containment and hot-pluggability.

## Open branches (next sessions)

- **(A) Crate & dependency-graph revision** — ✅ **closed 2026-06-24** (ADR-0009).
  Spine-inverted, process-aligned topology; full target tree recorded in the ADR.
- **(B) Strategy runtime & multi-topic-join framework** — subscribing to and
  *fusing* multiple streams (market data + news → Signal); the strategy↔Core
  seam; strategy sandboxing contract.
- **(C) Frontend / CLI design** — the MVP CLI Frontend: observability + operational
  control (no trading control — operator orders route through Signal→risk later);
  query channels (req/resp to Core & Supervisor) and telemetry topics.

## Parked sub-questions (don't lose these)

- Schema **evolution/versioning** of `#[repr(C)]` POD messages (ADR-0002).
- Bus trait's **loan-vs-own** contract (design to the borrowed/lifecycle-bounded
  case to keep zero-copy).
- Per-topic **delivery semantics** detail + backpressure / ring sizing.
- **Req/resp pattern** (registration, reconciliation queries, Frontend queries):
  the request is effectful/live-only, the response enters Core as an ordered input;
  Bus-capability vs side-channel TBD.
- **Event Log / repository backend**: parquet + DataFusion / DuckDB candidate
  (keep the log↔repository split from ADR-0009).
- **Snapshot** cadence and contents (recovery substrate).
- **Symbology** design: canonical identity (perm_id/OpenFIGI) + per-adapter
  mapping.
- `Price`/`Quantity` numeric needs per asset class (crypto/wei) — when to move
  the inner type off `rust_decimal`.
- **Deterministic client-order-id** generation scheme.
- **Core failover** (Aeron-Cluster-style hot standby) — future, documented in
  ADR-0005.
