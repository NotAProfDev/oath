# Crate topology: spine-inverted and process-aligned

The original workspace is a single-process monolith — `oath-engine` composes every
layer (`engine → {everything}`), with inter-layer trait edges (`risk → execution →
portfolio`). ADRs 0001–0008 commit instead to a single-host, multi-process,
event-sourced system, which inverts the graph: there is no top composer, only a
**bottom contract** every process depends on. We restructure the crates to match —
the dependency arrows point *inward to `oath-model`* (the canonical model + Bus +
Event Log spine), and the directory layout encodes the process boundaries.

Two naming rules make the structure self-describing:

- **`<subsystem>/api` is the trait crate**; `<subsystem>/<impl>` are
  implementations (backends or Policies). `api` replaces the old `*-core` suffix.
- **`core/` is reserved for the Core process.** Its single-writer loop is
  `core/kernel` (the **Kernel**); its trait hub is `core/api`. Top-level
  directories are processes/subsystems; crates nested under `core/` are Core
  internals — so an illegal reach from, say, `adapter/` into `core/kernel` is
  visible at a glance.

## Target layout

```
crates/
  model/                    oath-model              primitives + message payloads

  bus/
    api/                    oath-bus-api            Bus trait
    iceoryx2/ chronicle/…   oath-bus-*              backends (chronicle also impls event-log)
  event-log/
    api/                    oath-event-log-api      Event Log + Snapshot traits
    chronicle/ parquet/…    oath-event-log-*        backends (DataFusion/DuckDB/parquet — parked)
  persistence/              ── reserved ──
    api/                    oath-persistence-api    Repository trait
    …/                      oath-persistence-*       backends

  core/                     ── Core process ──
    api/                    oath-core-api           StateView, Decision, ActionSink,
                                                    RiskPolicy/ExecutionPolicy/Portfolio
    risk/                   oath-core-risk          RiskPolicy impl
    execution/              oath-core-execution     ExecutionPolicy impl
    portfolio/              oath-core-portfolio     Portfolio impl (generic, for now)
    kernel/                 oath-core-kernel        Kernel⟨R,E,P⟩ single-writer loop (lib)
    host/                   oath-core               bin: the Core process

  adapter/
    api/                    oath-adapter-api        harness + Broker/DataProvider traits
    net/
      api/                  oath-adapter-net-api    HTTP/WS traits
      reqwest/…             oath-adapter-net-*      backends
    ibkr/…                  oath-adapter-ibkr       bin: a venue

  strategy/
    api/                    oath-strategy-api       Strategy trait + host harness
    host/                   oath-strategy-host      bin: Strategy Node

  cli/                      oath-cli                bin: Frontend (MVP)
  supervisor/               oath-supervisor         bin: operational plane
```

## Considered options

- *Keep the monolith graph (`engine → {everything}`)* — rejected: it is the
  single-process composition ADR-0001 explicitly chose against.
- *Per-process top composers* — unnecessary: with binding-time pluggability
  (ADR-0007) each process binary composes itself from generic libraries; no shared
  composer crate is needed.

## Consequences

- **`oath-engine` is deleted.** Its composition role is gone; the Core process is
  the binary `oath-core` (at `core/host`), assembled from `core/kernel` + chosen
  Policies + chosen backends. Topology orchestration moves to `oath-supervisor`,
  kept out of Core for determinism (ADR-0005).
- **The Kernel is generic** (`Kernel<R, E, P>`, in `core/kernel`) and depends only
  on the trait hub `core/api`, never on a concrete Policy; the `oath-core` binary
  binds the concrete risk/execution/portfolio crates (ADR-0007/0008).
- **Adapter/Strategy traits live on their own side.** `Broker`/`DataProvider`
  (`adapter/api`) and `Strategy` (`strategy/api`) are called statically by their
  host harness; Core depends on neither — it shares only the canonical model over
  the Bus (ADR-0007).
- **`oath-ingest-core` is deleted.** Market data is canonical messages in
  `oath-model`, published by adapters, carried on the Bus.
- **Event Log and repositories split.** `event-log/api` (append-only, ordered,
  replayed — the recovery spine) is separated from the reserved `persistence/api`
  (keyed, queryable repositories: read-models, symbology, adapter dedup tables).
- **`net` moves under `adapter/`** (`adapter/net/api`) — adapters are its only
  user; Core and strategies speak only the Bus.
- **Two new process roles**: `oath-supervisor` (operational plane) and `oath-cli`
  (the first Frontend). Both sit outside Core's deterministic path and depend only
  on public `*-api` + `oath-model` + the Bus.
- The README dependency graph is updated when the restructure is implemented
  (one issue, one PR); **this ADR is the authoritative target until then.**
