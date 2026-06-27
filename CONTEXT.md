# OATH — Open Automatic Trading Hub

The ubiquitous language for a single-host, multi-process, backend-agnostic
trading engine. This file is a glossary only — no implementation details.

## Domain primitives

**Symbol**:
OATH's canonical identifier for a tradable instrument, independent of any one
venue's ticker or internal id, so the same instrument offered by different
brokers collapses to a single `Symbol` (e.g. via perm_id / OpenFIGI). `Symbol`
is instrument _identity_ — used for positions, Signals, and risk — and is **not**
a data-stream routing key: market-data streams are additionally keyed by their
[Source], because the same `Symbol` priced by two Sources is two distinct streams.
_Avoid_: ticker, instrument, contract (for the canonical form).

**Source**:
The Broker or Data-Provider Adapter that produced a given data stream — part of a
market-data topic's routing key, never part of instrument identity. The same
`Symbol` carried by two Sources is two distinct streams (different prices,
timestamps, and gaps); a consolidated/NBBO view is a _derived_ stream, never raw
per-Source topics conflated.
_Avoid_: venue, feed, provider (for this routing-key role).

**Price**:
The value per unit of an instrument, expressed in its quote currency. Can be
**negative** (e.g. spreads, or a commodity in backwardation gone sub-zero). Its
decimal granularity is a property of the **instrument** (its tick size), not of the
price value itself.
_Avoid_: cost, rate, level.

**Quantity**:
A **magnitude** — a non-negative amount of an instrument. Direction is never
carried by a Quantity; it lives in [Side], and net signed exposure is a property of
a Position derived from Side + Quantity.
_Avoid_: size, amount, volume; a "signed quantity".

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
order when conditions change — not only in response to a strategy. Never hosts
strategies in-process; they always run in separate Strategy Nodes.
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

**Strategy**:
A unit of user-authored logic, hosted in a Strategy Node, that consumes market
data and enrichment off the Bus and proposes Signals. It never trades directly —
Core decides and acts. A _deterministic_ Strategy folds purely over its Bus
inputs; an _effectful_ Strategy may also do ad-hoc I/O.
_Avoid_: algo, bot, model, signal generator.

**Strategy Node**:
A separate process — never Core itself — hosting one or more user strategies,
isolated so that a strategy fault cannot crash the Core. Communicates with Core
only over the Bus.
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

## Execution environments

**Environment**:
An isolated instance of the OATH topology — its own Core, Event Log,
portfolio/risk state, execution-adapter binding, data feeds, and Bus namespace —
so that several can run on one host without their orders, fills, positions, or
logs colliding. Its mode is its data feed × execution backend (e.g. live feed ×
live account); all its feeds share one temporal profile (real-time, delayed-by-D,
or historical).
_Avoid_: instance, deployment, tenant, session.

**Simulated Broker**:
A Broker-adapter backend that fills Orders internally against the Environment's
own market-data feed instead of routing to a real venue. The execution backend
for Backtest and Shadow.
_Avoid_: mock, fake, matching engine, paper (paper uses a real broker account).

**Shadow**:
An Environment running live (or delayed) data through a Simulated Broker,
alongside Live or Paper, to test a Strategy on real-time data with no capital at
risk. It fills against the exact data the Strategy saw, exercising the model
end-to-end.
_Avoid_: dry-run, what-if, sim.

**Paper Trading**:
An Environment routing real Orders to a broker's paper (demo) account — real
broker-side execution, no real money, often on the broker's delayed market data.
_Avoid_: demo, sandbox, simulation.

**Live Trading**:
An Environment routing real Orders to a broker's live account, with real money at
risk.
_Avoid_: production, real-money mode.

## Messages & decisions

**Signal**:
A Strategy's proposal of a _desired target_ — the position or exposure it wants
in a `Symbol` — submitted to Core for a decision, never an Order. Idempotent and
nettable: Core reconciles actual → target across strategies under risk, deciding
whether, when, and how much to act. Carries the as-of freshness it was decided
under and the proposing Strategy's identity.
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

**Emergency Halt**:
An operator-tripped switch that puts Core's Risk Engine into cancel-all / flatten
mode — operational safety, not operator trading: it picks no Symbol, Side, or
Quantity, only invoking risk's existing authority (a control of the risk loop, not
an Order). The Supervisor performs the effectful trip and emits a logged Core
input, so it is deterministic and replayable.
_Avoid_: kill-switch, panic button, stop (for the process-lifecycle sense).

## Observability

**Business State**:
The continuous, observable state of the trading business — positions, P&L,
exposure — projected from Core's canonical fold and pushed as one coalesced,
latest-value snapshot, stamped with the Event Log sequence it reflects so
observers detect a stalled producer rather than a steady value. Rendered
directly, never re-folded. The observable subset of Core state — distinct from
the recovery Snapshot (full internal state) and from Telemetry (machinery, not
business).
_Avoid_: portfolio view, book, blotter, dashboard state.

**Domain Event**:
A discrete, must-deliver fact Core's fold produced — order placed, fill applied,
signal admitted/rejected, breach fired/cleared, cancelled-by-risk, alert —
carried on one durable, ordered narrative stream for observers and audit. It
surfaces the outcome of a Decision as a derived fact; the Decision itself stays
internal and never reaches the Bus. Ordered, never coalesced.
_Avoid_: notification, log entry, message, decision.

**Telemetry**:
Operational metrics of the machinery, not the business — per-topic throughput
(messages/sec), signal- and order-generation rates, latencies, queue depths,
process health. Sampled on wall-clock outside Core's deterministic fold, so it
is not canonical state and is not Event-Log sequenced. Coalescing latest-value,
like Business State, but instrumentation rather than business fact.
_Avoid_: metrics, stats, monitoring data.

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

**Backtest**:
Running live Strategy code against recorded historical market data and
enrichment, fed in timestamp order through the same Core wired to a Simulated
Broker, to evaluate what it would have done. A deterministic Strategy's Backtest reproducibly matches live;
an effectful Strategy can still be backtested, but parity and freedom from
lookahead are then the author's responsibility, not the framework's. Distinct
from Replay: Replay re-feeds Core's _logged_ inputs and never re-runs strategies,
whereas a Backtest _regenerates_ Signals from the Strategy.
_Avoid_: simulation, paper trading, replay (for this).

## Transport

**Bus**:
The transport over which processes exchange canonical messages. Backend-agnostic
(e.g. shared-memory zero-copy, Unix sockets, Kafka) behind a single trait.
_Avoid_: queue, channel, broker (reserved for the venue role above).

**LatestValue**:
A Bus delivery class: a **keyed store** of the latest value(s) per instance-key
(depth ≥ 1), lossy and overwrite-allowed and **per-key isolated**, so a busy key
never starves a quiet one. Read **by key** (with change-notification), never as a
filtered firehose. Models current price/quote, order-book snapshot, position/P&L.
_Avoid_: cache, snapshot (reserved for recovery), drop-to-latest (as a noun).

**Reliable**:
A Bus delivery class: an **ordered stream** in which no message is silently
dropped — a full queue yields an explicit error, never a block and never a silent
loss. Models every tick/fill/order/Domain Event.
_Avoid_: durable (durability is the [Event Log]'s concern, not the Bus's),
guaranteed.
