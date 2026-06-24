# Single-owner Kernel with stateless Policies over a StateView

Core's decision components — risk, execution, portfolio — hold **no state of
their own and do not depend on one another**. The single-writer **Kernel**
(ADR-0005) owns *all* canonical state; each component is a **stateless Policy**
the Kernel invokes as a decision function over a read-only `StateView` of that
state. This extends ADR-0005's single-owner argument one level down: the same
reason cross-symbol invariants forced a single writer *between* processes (rather
than sharding) forbids distributing state ownership *within* Core across
co-owning components.

## Considered options

- *Component co-owners* — portfolio owns positions, execution owns the open-order
  table, risk owns limit state; the Kernel sequences calls but each mutates its
  own slice. Rejected: a global invariant (e.g. buying power) spans all three
  owners, so the check must either read across them (which re-creates the
  `StateView` anyway) or be hoisted into the Kernel — and distributed mutation
  widens the determinism-audit surface and complicates Snapshot. It re-litigates
  ADR-0005 one level down.
- *Kernel sole owner; components are stateless Policies over a `StateView`* —
  chosen.

## Consequences

- The old `risk → execution → portfolio` dependency edges vanish: a Policy
  depends only on the `StateView` contract, never on a sibling. Portfolio is
  modelled as *fold logic* (how a Fill updates positions) plus read accessors, not
  as a state owner.
- "Swappable" here is a **Policy swap** (different risk *rules*, a different
  execution *algo*), not a backend swap — bound at compile time per ADR-0007. A
  swapped-in Policy holds a read-only view and therefore *cannot* corrupt
  canonical state.
- A Policy that needs private state declares an associated `Private` type the
  Kernel **custodies** and the Policy mutates **in place** (`&mut Self::Private`)
  — keeping it inside the one Snapshot and one fold, and avoiding the per-event
  allocation a functional `-> NewState` return would impose on the Disruptor
  decision stage (ADR-0005).
- A Policy never *acts*; it emits a **Decision** — the set of intended actions for
  one input (admit/shape/reject a Signal; cancel/amend/flatten resting orders).
  The Kernel drains the Decision and performs every action — it is the sole actor,
  with effects suppressed on replay. To avoid per-event allocation the Policy
  pushes actions into a Kernel-provided, reused **sink** (`&mut ActionSink`)
  rather than returning an owned collection — the same in-place discipline as
  `Private`.
- `StateView` is a single read-only trait, **generic-dispatched** (monomorphized,
  zero-cost, fixture-implementable for tests), exposing all canonical read-state.
  It must expose **no mutation or interior mutability** and **no nondeterministic
  iteration** (keyed/ordered accessors only), so replay cannot diverge.
- `StateView`, `Decision`, and the Policy traits are a *Core-subsystem* contract:
  they live in a Core-internal hub crate, not in `oath-model` (reserved for
  cross-subsystem / over-the-Bus types).
