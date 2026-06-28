# From-scratch architecture — grilling summary (2026-06-23)

**Status:** Branch (A) — crate & dependency-graph revision — **closed 2026-06-24**
(ADRs 0007–0009). Branch (B) — strategy runtime & multi-topic-join — **closed
2026-06-25** (ADRs 0010–0013). Branch (C) — Frontend/CLI — **closed 2026-06-26**
(ADRs 0014–0019). **All architecture branches closed.** The **Bus-contract**
parked-question cluster — **closed 2026-06-27** (ADRs 0020–0022): trait delivery
classes × access patterns, two-level routing, and the `Reliable` order-path
failure model. **Numeric types** parked question — **closed 2026-06-27**
(ADR-0023): fixed-point always-`i128`, exact/analytical two-domain split,
instrument-sourced precision, checked money ops. **Symbology** parked question —
**closed 2026-06-27** (ADR-0025): self-_identifying_ `InstrumentId` (externally
anchored / venue-qualified fallback, never guess a collapse), off-wire `Instrument`
reference record, fixed-size name on the wire, deterministic rule + curated mapping,
logged resolution + lifecycle. It also reopened and refined **ADR-0011 → ADR-0024**
(an Environment binds **one-or-more same-safety-class** execution backends, so
cross-broker risk is in-fold; shadow/live/off is per-strategy **targeting**, not an
in-Core tag) and added glossary `Account` / `Position`.

## The question

"If we built OATH's scaffolding from scratch, would we do it differently?" Goal:
a high-performance, production-grade, secure, maintainable trading engine with
everything behind traits so backends are swappable.

**Short answer: yes, structurally.** The existing scaffolding assumes a
single-process, statically-composed monolith (`oath-engine` composes all layers
via `EngineBuilder`; dep graph `engine → {everything}`). The design we converged
on is a **single-host, multi-process, event-sourced system communicating over a
swappable message Bus**. The current crate graph is aimed at the monolith we
decided _not_ to build — revising it is open branch (A) below.

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
  later, _measured_ optimization. Replay = fold over the Event Log.
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
- **[ADR-0014](../../adr/0014-observability-three-planes-deterministic-boundary.md)** —
  Observability is three planes split on Core's deterministic boundary: **Business
  State** (continuous, in-fold, seq-stamped coalescing snapshot) + **Domain Events**
  (discrete, in-fold, durable curated narrative; Decisions stay internal, surfaced
  as derived facts) + **Telemetry** (technical, out-of-fold, per-process). Read
  path = push-spine + narrow query; observers never re-fold.
- **[ADR-0015](../../adr/0015-off-thread-split-egress-observable-outputs.md)** —
  Off-thread, split egress: the Kernel enqueues, a non-blocking forwarder
  publishes. Coalescing latest-value snapshot (one atomic slot, seq-stamped) +
  must-deliver durable Domain-Event channel + admission-bounded query channel.
  Memory-boundedness is structural; replay path byte-identical (null sink).
- **[ADR-0016](../../adr/0016-request-reply-over-bus-query-tiering.md)** —
  Req/reply is a thin correlation layer over the Bus's pub/sub (no side-channel, no
  native Bus method); one backend matrix. Queries tiered: push-spine → Kernel
  non-logged read → repository / Event-Log store. Frontend reads never hit the
  Event Log (only reconciliation responses do).
- **[ADR-0017](../../adr/0017-frontend-control-plane-operational-only.md)** —
  Frontend control is operational-only (lifecycle to Supervisor); no operator
  trading (deferred to a Signal→risk seam). The one order-affecting control is an
  emergency halt, modeled as a logged trip of risk's existing flatten authority.
- **[ADR-0018](../../adr/0018-frontend-architecture-scopes-library-session.md)** —
  Frontend = reusable `frontend-core` library + thin presentations (CLI first,
  TUI/web later). Two scopes: host (always-on Supervisor) + Environment (one Core
  namespace). Persistent interactive session + one-shot subcommands; one
  Environment at a time; switch is first-class; discovery is Supervisor-driven.
- **[ADR-0019](../../adr/0019-frontend-trust-boundary-host-os-mvp.md)** —
  MVP trust boundary = host OS (Bus segment/socket perms); no app-level auth.
  Identity/principal seam reserved; control already audited. Authn + authz tiering
  arrive with the networked Bus, where the threat model actually changes.

Glossary: **[CONTEXT.md](../../../CONTEXT.md)** (primitives, processes, messages,
persistence/recovery, transport).

## Validated against prior art

NautilusTrader independently uses nearly the same model (single-threaded
deterministic kernel, offloaded I/O, centralized cache, no sharding,
strategy→risk→exec). Key _deliberate_ divergence: Nautilus is single-process
with a non-swappable bus; OATH is multi-process with a swappable bus — paying
cross-process complexity to buy crash containment and hot-pluggability.

## Open branches (next sessions)

- **(A) Crate & dependency-graph revision** — ✅ **closed 2026-06-24** (ADR-0009).
  Spine-inverted, process-aligned topology; full target tree recorded in the ADR.
- **(B) Strategy runtime & multi-topic-join framework** — ✅ **closed 2026-06-25**
  (ADRs 0010–0013). Out-of-Core deterministic-fold strategies; capability-derived
  backtest fidelity; Environments & mode isolation; event-time fusion + parity/`L`;
  push framework, Signal-as-target, registration, fault isolation.
- **(C) Frontend / CLI design** — ✅ **closed 2026-06-26** (ADRs 0014–0019).
  Three-plane observability (Business State / Domain Events / Telemetry) on a
  push-spine + narrow query; off-thread split egress; req/reply over the Bus;
  operational-only control with halt-via-risk; `frontend-core` library + two
  scopes + persistent session; host-OS trust boundary for MVP.

## Parked sub-questions (don't lose these)

- Schema **evolution/versioning** of `#[repr(C)]` POD messages (ADR-0002).
- ~~Bus trait's **loan-vs-own** contract~~ — **resolved (ADR-0020)**: public RAII
  loan guard (degrading to owned), with mandated copy-out to owned `M: Copy` POD at
  the retention / thread-hand-off / `.await` boundaries.
- ~~Per-topic **delivery semantics** + backpressure / ring sizing~~ — **resolved
  (ADR-0020 / 0022)**: two classes (`LatestValue` keyed store / `Reliable` stream);
  `Reliable` overflow errors (never blocks, never silently drops) under a graduated
  order-path ladder; routing is two-level (ADR-0021).
- ~~**Req/resp pattern**~~ — **resolved (ADR-0016)**: req/reply is a thin
  correlation layer over the Bus (not a side-channel); Frontend read-queries are
  non-logged and tiered (push-spine → Kernel read → repository), while only a
  reconciliation _response_ enters Core as an ordered input. _Registration was a
  settled instance already (ADR-0013)._
- **Event Log / repository backend**: parquet + DataFusion / DuckDB candidate
  (keep the log↔repository split from ADR-0009).
- **Snapshot** cadence and contents (recovery substrate).
- ~~**Symbology** design: canonical identity (perm_id/OpenFIGI) + per-adapter
  mapping.~~ — **resolved (ADR-0025)**: **`InstrumentId`** = self-_identifying_
  canonical identity (externally-anchored ISIN/FIGI/OCC where it exists,
  venue-qualified fallback, **never guess a collapse**); **`Symbol`** demoted to venue
  ticker. Off-wire **`Instrument`** record keyed `(InstrumentId, Source)` (shared core +
  per-asset-class typed tail) is the single home for ADR-0023 precision. Wire form =
  fixed-size self-identifying name (Choice A; local-only interning). Mapping =
  deterministic versioned rule + curated overrides + cross-`Source` price-plausibility
  monitor. Resolution + lifecycle (immutable id + logged succession; corporate actions
  as Core inputs; time-versioned metadata) are **logged Core inputs**. _Also drove
  **ADR-0024** (multi-broker same-safety-class Environments; shadow = targeting) and
  added glossary `Account` / `Position`._
  - **New parked sub-questions:** fixed-size id **length** vs real IBKR contracts;
    **combo identity** (structural-encode vs leg-decompose); **central security-master**
    service (production-grade evolution of the resolution/curation seam — additive,
    does not change identity); **OpenFIGI** anchor-lookup fallback; the
    **corporate-action taxonomy** + succession UX; durable **`Instrument` store** as
    part of the Event-Log / repository backend decision.
- ~~`Price`/`Quantity` numeric needs per asset class (crypto/wei) — when to move
  the inner type off `rust_decimal`.~~ — **resolved (ADR-0023)**: drop
  `rust_decimal`; fixed-point **always-`i128`** on the wire (`Price` signed,
  `Quantity` unsigned magnitude + `Side`); **two-domain split** (exact `i128` /
  analytical `f64`, convert at the strategy boundary); precision is **instrument
  metadata**, raw-only wire; money ops are checked / no-bare-arithmetic / widen-to-256
  for notional; layered float-determinism scope (refines ADR-0012).
- ~~**Deterministic client-order-id** generation scheme.~~ — **resolved
  (ADR-0026)**: three ids by role — **Order Id** (internal, lifecycle-stable),
  **Order Instruction Id** (per place/amend/cancel; the dedup + reconciliation join
  key), **Broker Order Id** (venue-assigned, learned). Internal ids are
  derived-deterministic `(EnvironmentId, generation, counter)`; `generation` is
  bumped once per boot and logged as an `IncarnationStarted` marker (reproduced by
  folding, never recomputed). Env-wide order-counter; `Account`/`Source` not in the
  id. Two many-to-one indexes (instruction→order, broker→order). Chaining is
  Core-owned (`supersedes` in the canonical `OrderInstruction`); the adapter renders
  to the venue (FIX `ClOrdID`/`OrigClOrdID`/`OrderID`). Refines ADR-0006.
  - **New parked sub-question:** the **amend-before-ack** edge (amending an [Order]
    whose place is not yet acknowledged, so no `Broker Order Id` exists) — order
    state-machine territory, separate from identity.
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
