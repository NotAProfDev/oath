# Frontend control plane: operational-only, kill-switch via risk authority

The Frontend's control half is **operational, not trading**. Lifecycle commands —
start / stop / restart an Adapter or Strategy Node, start / stop an Environment,
admit / evict a Strategy — go to the **Supervisor** over `supervisor.control.req`
(ADR-0016) and touch only topology, never the order path. The operator **cannot**
place, cancel, or amend individual orders or set targets; discretionary operator
trading is deferred to a future **Signal→risk seam** (the operator's order becomes
a Signal, decided by Core under risk like any other — ADR-0004 / ADR-0013), so no
path ever bypasses risk. The one order-affecting control in MVP is an **emergency
halt**, modeled not as operator trading but as a **trip of the Risk Engine's
existing cancel-all / flatten authority** (ADR-0004): the operator picks no
instrument and no size, only trips the switch. Mechanically it follows ADR-0013's
registration template — the Supervisor durably records the "halt as-of seq N"
fact into Core's Event Log before (or as part of) the effectful trip — so the
halt is deterministic, replayable, and attributable even across a crash.

## Considered options

- _Kill-switch as direct order cancellation by the operator_ — rejected: that is
  operator trading control (it issues order actions), bypasses the risk loop that
  owns cancel / amend / flatten authority (ADR-0004), and adds a second, unlogged
  actor on the order path against ADR-0008's single-actor Kernel.
- _Kill-switch out of MVP (Supervisor-only control)_ — rejected: a process halt
  leaves resting orders live at the broker and positions unmanaged — more
  dangerous, not less. A system that trades real money must be able to flatten
  before it can be called done; observability without a safety stop is the wrong
  order of priorities.
- _Operator discretionary orders in MVP_ — deferred (not rejected): routing
  operator orders through Signal→risk is the correct eventual design, but it
  depends on the Signal admission path and is outside the Frontend MVP's scope.

## Consequences

- **No path bypasses risk.** Every order-affecting action — strategy Signals and
  the operator's emergency halt alike — is decided and performed by Core's Kernel
  under risk; the Frontend never emits Orders.
- **One control mechanism.** Lifecycle and halt are both req/reply to the
  Supervisor (ADR-0016); halt additionally produces a logged Core input via the
  ADR-0013 template. No new transport or pattern.
- **Halt is auditable and replayable** like admission: "what was halted, as of
  which seq" is reconstructable from the Event Log.
- **The Signal→risk seam is the single extension point** for all future trading
  control (operator orders, manual target overrides), keeping the Frontend's
  control surface from ever growing its own order path.
