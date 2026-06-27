# Risk as a continuous, autonomous control loop

The Risk Engine is a continuous control loop with veto/cancel/amend authority
over every order, not a one-shot pre-trade gate. It surveils all live state and
may act autonomously — for example, cancelling a resting order already on a
venue's book because a correlated fill changed the exposure or capital is now
better deployed elsewhere. Strategies only *detect* and propose **Signals**;
Core *decides and acts*.

## Consequences

- The `risk-core` / `execution-core` traits are an event-in / command-out state
  machine, not a `check(order) -> verdict` function.
- Risk is un-bypassable: a Strategy cannot reach a Broker directly; it emits a
  Signal that Core adjudicates.
- Core must hold and continuously update the full state risk depends on, which
  motivates the single-writer design under discussion in ADR-0005.
