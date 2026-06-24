# OATH — Open Automatic Trading Hub

The ubiquitous language for a single-host, multi-process, backend-agnostic
trading engine. This file is a glossary only — no implementation details.

## Domain primitives

**Symbol**:
OATH's canonical identifier for a tradable instrument, independent of any one
venue's ticker or internal id, so the same instrument offered by different
brokers collapses to a single `Symbol` (e.g. via perm_id / OpenFIGI).
_Avoid_: ticker, instrument, contract (for the canonical form).

**Price**:
The value per unit of an instrument, expressed in its quote currency.
_Avoid_: cost, rate, level.

**Quantity**:
An amount of an instrument.
_Avoid_: size, amount, volume.

**Side**:
The direction of an order or trade — buy or sell.
_Avoid_: direction, way, sign.

**Timestamp**:
A point in time, always UTC, with no timezone or offset attached.
_Avoid_: datetime, date, clock time.

## Processes & topology

**Adapter**:
A process that connects OATH to exactly one external venue, translating
between that venue's representation and OATH's canonical model. The translation
is the adapter's responsibility and never leaks inward (anti-corruption layer).
_Avoid_: connector, plugin, integration.

**Broker**:
An adapter to a trading venue that provides everything needed to trade an
instrument: market data, instrument/reference data, and the order path (place
orders, receive fills).
_Avoid_: exchange, venue (when meaning the integration).

**Data Provider**:
An adapter to a source of enrichment data a broker does not supply — news,
social, macro/country data — used to improve trading and risk decisions.
_Avoid_: feed, vendor, source.

**Core**:
The process and central decision authority. Holds all live state needed for
portfolio and risk decisions (positions, open orders, exposure) and
continuously surveils it. Acts autonomously — e.g. cancels or amends a resting
order when conditions change — not only in response to a strategy. Initially
also hosts strategies.
_Avoid_: server, hub, main, master, cache.

**Kernel**:
The single-writer heart of Core: the one logical thread that owns all canonical
state and is the only thing permitted to mutate it. Core's state is a pure fold
the Kernel applies, and decision Policies run inside it over a read-only view of
that state.
_Avoid_: engine, main loop, scheduler, reactor.

**Risk Engine**:
The component within Core that continuously evaluates live state and holds
veto/cancel/amend authority over every order. A control loop, not a one-shot
pre-trade gate.
_Avoid_: risk check, validator, guard.

**Policy**:
A stateless, compile-time-selected decision rule the Kernel invokes over a
read-only view of state — for example a risk Policy (cancel/amend rules) or an
execution Policy (how to work an order). Carries configuration, not state; any
private state it needs is custodied by the Kernel.
_Avoid_: rule engine, handler, check, plugin, strategy (reserved for the
user-facing Strategy).

**Strategy Node**:
A process hosting one or more user strategies, isolated so that a strategy
fault cannot crash the Core.
_Avoid_: worker, runner, bot.

**Supervisor**:
The operational-plane process that boots and watches a host's topology — starts
the Bus, spawns Core, Adapters, and Strategy Nodes, runs health checks and
throughput monitoring, and restarts failed processes. Purely effectful; it never
participates in Core's deterministic decision path.
_Avoid_: orchestrator, launcher, manager, daemon, conductor.

**Frontend**:
An external process that observes — and may control — the running hub from
outside Core's deterministic path: positions, orders, P&L, process health. The
CLI is the first Frontend; TUIs, web, and desktop UIs are later ones. Depends only
on the public message model, Bus, and query interfaces.
_Avoid_: UI, dashboard, console, client.

## Messages & decisions

**Signal**:
A strategy's proposal to trade (which `Symbol`, `Side`, size/urgency),
submitted to Core for a decision. Not itself an order — Core decides whether,
when, and how much to act.
_Avoid_: order, intent, trade (from a strategy).

**Decision**:
What a Policy returns to the Kernel for one input: the set of intended actions —
admit/shape/reject a Signal, or cancel/amend/flatten resting orders. The Policy
decides; the Kernel performs the actions. Internal to Core, never sent on the Bus.
_Avoid_: verdict, judgment, command, ruling.

**Order**:
An instruction Core sends to a broker to buy or sell, after the Risk Engine
approves. Carries the freshness/validity context it was decided under.
_Avoid_: trade, transaction.

**Fill**:
A partial or complete execution of an order, reported by a broker.
_Avoid_: execution, trade, transaction.

## Persistence & recovery

**Event Log**:
The persisted, totally-ordered record of every input Core consumed, in the
order it consumed them. Core's state is a pure fold over it.
_Avoid_: journal, history, audit log, WAL.

**Snapshot**:
A point-in-time capture of Core's full state, tagged with its position in the
Event Log, so recovery can resume without replaying from the beginning.
_Avoid_: checkpoint, dump.

**Replay**:
Re-feeding the Event Log through the identical fold to reconstruct state, with
all external side effects suppressed. The basis of both recovery and
backtesting.
_Avoid_: rerun, simulation (for the deterministic re-feed specifically).

## Transport

**Bus**:
The transport over which processes exchange canonical messages. Backend-agnostic
(e.g. shared-memory zero-copy, Unix sockets, Kafka) behind a single trait.
_Avoid_: queue, channel, broker (reserved for the venue role above).
