# Strategy runtime: push framework, Signal-as-target, registration, fault isolation

One **push-based host framework** drives every strategy — it owns the loop, the
event-time merge, the latest-value view, the ingestion log, and lifecycle; the only
per-flavour difference is which capability context is handed in (`DetCtx`, sync /
`IoCtx`, async). A **Signal** is an **idempotent** proposal of a _desired target_
(position / exposure) for a `Symbol` — never an order — carrying the as-of freshness
it was decided under and the proposing `StrategyId`; Core nets targets across
strategies and reconciles actual → target under risk (ADR-0004). A Strategy Node
**registers** via a two-part handshake: the Supervisor performs the effectful join
(authenticate, assign a restart-stable `StrategyId`, resolve symbology, attach to
the Environment's Bus namespace) and then emits an ordered **"Strategy admitted"**
record into Core's Event Log, so the active-strategy set and per-strategy
authorization / limits are deterministic, replayable state.

## Considered options

- _Pull / async loop for all strategies_ — rejected: it lets a strategy interleave
  I/O and own its timing, breaking determinism, and a `sleep`-based timer breaks
  Replay. Push with **framework-injected timers** (a timer fires as an event in the
  merge stream) keeps even time-driven strategies deterministic and backtestable.
- _Signal as an order-proposal_ ("buy 100 now") — rejected as the primitive: not
  idempotent (a dropped or duplicated proposal corrupts the position), cannot be
  netted across strategies, and fights the risk control loop (ADR-0004). A strategy
  that thinks in orders expresses target = current + delta; raw-alpha with
  Core-side portfolio construction is a possible additive Signal _kind_ later.
- _Registration as Supervisor-only control-plane state_ — rejected: attribution and
  per-strategy limits are decision-relevant and must survive Replay; Supervisor-only
  state cannot be replayed, and Replay could not reconstruct which strategies were
  active when.

## Consequences

- **Two trait variants over one framework:** `fn on_event(&mut self, input, ctx:
  &mut DetCtx)` (deterministic, sync — no async runtime on the hot path) and `async
  fn on_event(&mut self, input, ctx: &mut IoCtx)` (effectful). Both receive the
  merged event-time input + latest-value view and emit Signals into a reused sink
  (ADR-0008 discipline).
- **Hot-plug is trivial:** admitting or evicting a strategy is just another folded
  Core input — no Core restart (ADR-0001). This is the parked req/resp pattern's
  clean instance: the join _request_ is effectful and Supervisor-only; the _fact_
  ("admitted as of event-time T") enters Core as an ordered input.
- **Fault isolation.** A deterministic strategy is never fed lossy input — a slow
  one lags and its stale Signals are freshness-rejected, with the Supervisor
  evicting a chronic laggard (a logged deregistration). Panics and leaks are
  contained by process isolation (ADR-0001). Signal floods are largely defanged by
  idempotent targets (Core coalesces to the latest target per symbol) plus a
  per-strategy host rate-limit. Authorization breaches are rejected by Core's
  per-strategy limits.
- **Isolation granularity is operator-tunable:** one strategy per node by default
  (full isolation, independently restartable); native co-hosting as a trusted
  opt-in for density (shared fate); WASM sandboxing (Extism) later for density
  _with_ per-strategy isolation at 1000s-of-strategies scale.
