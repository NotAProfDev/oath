# Reliable order-path failure: graduated ladder, two halt modes, drain-fast adapter

When Core cannot publish an Order because the `Reliable` order topic is full
(ADR-0020), the response is a **graduated ladder**, not an immediate global halt:
**normal → telemetry-visible pressure → degraded/probing → scoped execution-path
safe-hold**. The first `Err(Full)` puts Core into a *degraded* state — a fold
state-transition, **never a blocked Kernel thread**: it withholds new Orders, fires
an **async health-probe** to the adapter (on a control topic, so it sends even
while the order ring is full), and waits in-state — bounded by an injected-clock
timer — for the ring to drain or the probe to reply. Only on confirmed timeout does
it declare the channel dead and enter a **scoped safe-hold** (stop emitting for
that one execution path, freeze, alert; the rest of the host runs on). On recovery,
held Orders are **freshness-re-checked** against their validity context before
sending — a stale Order is **dropped by Core's decision**, which is not a transport
drop and does not violate `Reliable`.

This rests on a **mandatory broker-adapter capability** (amending ADR-0006): the
order adapter **drains the Bus fast into its own internal, rate-limited pending
buffer** and paces venue submission from there — so a full Bus ring reflects
*adapter liveness*, not venue rate-limiting, making "full ⇒ wedged" honest.

It also **splits ADR-0017's halt into two modes**, because an overflow halt would
flatten through the very channel that is wedged:

- **Risk-trip halt** (broker reachable): the ADR-0017 emergency-halt-via-risk works
  — cancel/flatten through the live order path.
- **Channel-death halt** (the cancel path is itself the fault): cannot flatten — it
  can only stop-emitting, freeze, alert, and **defer** flatten/reconcile to
  reconnect-reconcile (ADR-0006). The halt protocol must distinguish the two; their
  available actions differ.

## Considered options

- _Immediate global halt on overflow_ — rejected: a full queue is local to one
  (Environment × broker), not a host-wide panic, and an unconfirmed `Err(Full)` may
  be transient — hence the probe.
- _Bounded retry / spin on the Kernel thread_ — rejected: blocks the single-writer
  and keeps generating un-sendable Orders.
- _A dedicated order fault channel_ — rejected: relocates the bound and adds a
  second order-delivery path that complicates reconciliation (ADR-0006).

## Consequences

- **The Kernel never blocks**; "wait" is a fold state, the probe is async (its
  reply/timeout are logged Core inputs), so the whole ladder is deterministic and
  replayable.
- **Telemetry is the early-warning plane** (ADR-0014): per-topic queue depth and
  fill-vs-drain rate, per adapter, surface pressure before any ring fills.
- **Ring depth is a false-positive-fault threshold**, not just a memory knob: bias
  the order ring shallow (fast wedge detection, low in-flight exposure) and put
  burst tolerance in the adapter's internal buffer. Depth is deployment config;
  only the delivery class is topic-static.
