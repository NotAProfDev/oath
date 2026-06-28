# OATH — Open Automatic Trading Hub

The ubiquitous language for a single-host, multi-process, backend-agnostic
trading engine. This file is a glossary only — no implementation details.

## Domain primitives

**InstrumentId**:
OATH's canonical, **venue-independent identity** for a tradable instrument,
independent of any one venue's ticker or internal id, so the same instrument
offered by different brokers collapses to a single `InstrumentId` — anchored, where
one exists, on an external standard (FIGI/ISIN-level), and **venue-qualified** where
no venue-independent identity exists, but always _normalized_, never a raw broker
string. It is the key for [Position]s (together with [Account]), [Signal]s, and
risk, and is **not** a data-stream routing key: market-data streams are
additionally keyed by their [Source], because the same `InstrumentId` priced by two
Sources is two distinct streams. **Self-identifying** — a stable, standards-based
(e.g. ISIN/FIGI/OCC) or deterministically-derived denotation that needs **no
OATH-private id registry** to say _which_ instrument it is, and never drifts, so the
Event Log stays interpretable forever. This is distinct from _self-explaining_: the
attributes (tick, currency, underlying, strike) still live in the [Instrument]
record, which is reference data _about_ an already-identified instrument, **not** a
fragile id↔meaning table.
_Avoid_: symbol, ticker, instrument, contract (for the identity).

**Symbol**:
The human-facing **ticker/label** for an instrument as a venue names it ("AAPL",
"ESM4") — a display attribute carried on the [Instrument] record, **never** the
identity (that is [InstrumentId]). Two venues may use different Symbols for the same
`InstrumentId`, and the same Symbol string can mean different instruments at
different venues — which is exactly why it cannot be the identity.
_Avoid_: identifier, id, key (for this label sense).

**Source**:
The Broker or Data-Provider Adapter that produced a given data stream — part of a
market-data topic's routing key, never part of instrument identity. The same
[InstrumentId] carried by two Sources is two distinct streams (different prices,
timestamps, and gaps); a consolidated/NBBO view is a _derived_ stream, never raw
per-Source topics conflated.
_Avoid_: venue, feed, provider (for this routing-key role).

**Instrument**:
The resolved _reference-data_ record for an [InstrumentId] — its [Symbol] (the
venue ticker), tick size, lot/min size, multiplier (contract size), quote currency,
asset class, and later expiry/strike/right/underlying. It is **not** identity (that
is [InstrumentId]) and never travels on the wire or the Event Log: ADR-0023 keeps
[Price]/[Quantity] precision-free raw `i128`/`u128`, and the `Instrument` is the
single home for the precision used to interpret them. The adapter **resolves it once
at the boundary** from a [Source]'s symbology / contract details, caches it, and any
consumer needing precision (order emission, display) looks it up. Resolution is keyed
by `InstrumentId` **per [Source]** — contract facts like tick can differ across
venues, so the same `InstrumentId` from two Sources may resolve to two `Instrument`s,
exactly as it is two market-data streams.
_Avoid_: contract, security, product (for this record); do not conflate with
[InstrumentId] (identity) or [Symbol] (ticker).

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

**Account**:
A specific trading account at a [Broker] that owns [Position]s and receives
[Fill]s — the unit OATH settles, margins, and flattens against. Its identity is a
**normalized composite that includes the [Source]**, because account ids are only
unique within a broker (account `U123` at broker A is unrelated to `U123` at
broker B), and one Broker may expose **several** Accounts. There is no cross-broker
account consistency.
_Avoid_: portfolio, wallet, login, subaccount (for the canonical term).

**Position**:
The held exposure in one [InstrumentId] at one [Account] — a [Quantity] magnitude
plus the [Side] that signs it, with signed exposure and average price derived. Keyed
by **`(Account, InstrumentId)`** and **never netted across Accounts**: a long at one broker
and a short at another are two Positions you must flatten separately, not a flat
zero. Net exposure across Accounts, brokers, or asset classes is a **derived
roll-up**, never a stored Position.
_Avoid_: holding, balance, inventory (for the canonical term).

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
portfolio/risk state, data feeds, and Bus namespace — so that several run on one
host without their orders, fills, positions, or logs colliding. An Environment
occupies **one cell of the mode matrix**: a single **temporal profile** (real-time,
delayed-by-D, or historical) × a single execution **safety-class** (Simulated,
Paper, or Live). Within that cell it may bind **one or more execution backends of
the same safety-class** (e.g. two Live brokers), so cross-broker [Position]s and
risk are evaluated in **one Core** over the canonical [InstrumentId]. A differing
temporal profile or safety-class **always** forces a separate Environment (a
Simulated or Paper [Fill] must never perturb Live risk); same-cell books may still
be split into separate Environments **by choice** for risk isolation. Brokers
co-bound into one Environment **share its fate** — a Core fault or [Emergency Halt]
touches all of them.
_Avoid_: instance, deployment, tenant, session.

**EnvironmentId**:
The stable, operator-assigned identity of an [Environment] — the same handle that
names its Bus namespace — recorded at genesis so it is replay-stable, and
administered unique across any Environments that could target the same [Broker]
[Account]. It prefixes order identities so two Cores can never collide at a shared
venue.
_Avoid_: name, tag, instance id.

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
in an [InstrumentId] — submitted to Core for a decision, never an Order. Idempotent and
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
The logical buy/sell order Core works at a [Broker], identified by its [Order Id]
and stable across a lifecycle of [Order Instruction]s (place/amend/cancel) until it
reaches a terminal state (filled/cancelled/rejected). Core sends it only after the
Risk Engine approves; it carries the freshness/validity context it was decided
under.
_Avoid_: trade, transaction.

**Order Instruction**:
A single command Core issues against an [Order] — _place_ (open), _amend_ (modify
price/quantity), or _cancel_. Each is identified by its own [Order Instruction Id]
and supersedes the previous instruction on the same Order. The unit the Bus carries
from Core to a [Broker] adapter.
_Avoid_: order modification, request, message (for this command).

**Order Id**:
Core's stable, internal identity for an [Order] — constant across its whole
lifecycle of [Order Instruction]s. Derived deterministically so [Replay] regenerates
it identically; never sent on the wire. The anchor every per-instruction and
broker-assigned id resolves back to.
_Avoid_: client order id (that is per-instruction), broker order id (that is the
venue's).

**Order Instruction Id**:
The identity of one [Order Instruction] — unique per instruction, reused only on
retransmission of the same instruction. Derived deterministically (so [Replay]
regenerates it) and the join key for idempotent submission and crash
reconciliation: the broker's dedup key and the "what happened to this?" question.
The adapter renders it to the venue's per-message id (e.g. FIX `ClOrdID`).
_Avoid_: client order id (ambiguous — it is per-instruction, not per-order),
message id, event id (reserved for [Domain Event]).

**Broker Order Id**:
The venue-assigned identity for an [Order], learned from the broker's
acknowledgements — a logged Core input, so replay-stable _as data_, not derived. One
[Order] may accumulate several across its life, since some venues re-issue it on
each amend. Used for venue-keyed queries and cross-checking the broker's books.
_Avoid_: order id (that is Core's internal id), exchange ref, venue id.

**Fill**:
A partial or complete execution of an order, reported by a broker.
_Avoid_: execution, trade, transaction.

**Emergency Halt**:
An operator-tripped switch that puts Core's Risk Engine into cancel-all / flatten
mode — operational safety, not operator trading: it picks no InstrumentId, Side, or
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
