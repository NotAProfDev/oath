# From-scratch architecture — grilling summary (2026-06-23)

**Status:** Branch (A) — crate & dependency-graph revision — **closed 2026-06-24**
(ADRs 0007–0009). Branch (B) — strategy runtime & multi-topic-join — **closed
2026-06-25** (ADRs 0010–0013). Branch (C) Frontend/CLI remains.

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
  Single-writer, event-sourced, deterministic Core. MVP = single-threaded kernel +
  offloaded async I/O (NautilusTrader-validated); disruptor pipeline is a
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
- **[ADR-0010](../../adr/0010-strategies-out-of-core-deterministic-folds.md)** —
  Strategies run only in Strategy Nodes (never in Core; supersedes ADR-0001
  co-location), wholly outside Core's deterministic boundary: a Strategy is a
  deterministic fold over Bus inputs + injected clock/seed, external data via Data
  Providers. Backtest-safety is a capability-derived (`DetCtx`/`IoCtx`) _fidelity
  label_, not a gate.
- **[ADR-0011](../../adr/0011-execution-environments-mode-isolation.md)** — Trading
  mode = _(data feed × execution backend)_; the Simulated Broker is a Broker-adapter
  backend, so Backtest/Shadow/Paper/Live are one matrix and Core + Strategy are
  mode-agnostic. Each mode is an isolated **Environment** (own Core, Event Log,
  state, execution adapter, Bus namespace), temporally homogeneous; the order path
  is never shared.
- **[ADR-0012](../../adr/0012-strategy-input-fusion-event-time-parity.md)** —
  Framework delivers an event-time-ordered merge + latest-value view; per-Environment
  lateness bound `L` (`L=0` live, zero added latency). Ingestion-order logging gives
  bit-exact session replay; fresh backtest uses event-time + `L` (parity above `L`).
  Late events marked-and-counted, never dropped.
- **[ADR-0013](../../adr/0013-strategy-runtime-push-signal-target-registration.md)** —
  One push framework (`DetCtx` sync / `IoCtx` async; timers as injected events). A
  Signal is an idempotent _desired target_ + freshness + `StrategyId`. Registration
  is a two-part handshake: Supervisor joins effectfully, then a logged "Strategy
  admitted" Core input makes the active-strategy set + limits deterministic. Fault
  isolation: freshness-reject + evict laggards; one-strategy-per-node default.

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
- **(B) Strategy runtime & multi-topic-join framework** — ✅ **closed 2026-06-25**
  (ADRs 0010–0013). Out-of-Core deterministic-fold strategies; capability-derived
  backtest fidelity; Environments & mode isolation; event-time fusion + parity/`L`;
  push framework, Signal-as-target, registration, fault isolation.
- **(C) Frontend / CLI design** — the MVP CLI Frontend: observability + operational
  control (no trading control — operator orders route through Signal→risk later);
  query channels (req/resp to Core & Supervisor) and telemetry topics.

## Parked sub-questions (don't lose these)

- Schema **evolution/versioning** of `#[repr(C)]` POD messages (ADR-0002).
- Bus trait's **loan-vs-own** contract (design to the borrowed/lifecycle-bounded
  case to keep zero-copy).
- Per-topic **delivery semantics** detail + backpressure / ring sizing.
- **Req/resp pattern** (reconciliation queries, Frontend queries): the request is
  effectful/live-only, the response enters Core as an ordered input; Bus-capability
  vs side-channel TBD. _Registration is now a settled instance (ADR-0013)._
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
- **Strategy sandbox** (Branch B): MVP relies on process isolation (Strategy Node)
  and the capability-derived determinism contexts (`DetCtx`/`IoCtx`). Physical
  WASM isolation for untrusted/effectful strategies is later hardening — candidate
  runtime **Extism** (<https://github.com/extism/extism>), whose host-function
  capability model maps onto the `DetCtx`/`IoCtx` boundary. WASM is also the
  density answer at scale: many sandboxed strategies in one process with
  per-strategy fault isolation (process-per-strategy doesn't reach 1000s of
  strategies on one host).
- **Simulated Broker fill model** (ADR-0011): fill-at-touch vs queue-position vs a
  slippage/latency model — governs Backtest/Shadow fidelity.
- **Delay-alignment mechanism** (ADR-0011): delay-relay vs adapter delayed-mode for
  aligning enrichment to a delayed market feed.
