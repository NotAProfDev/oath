# Single-writer, event-sourced, deterministic Core

Deterministic replay is a non-negotiable goal, and Core must enforce global
shared invariants (account buying power, global buying power, per-asset-class
risk) that are inherently cross-symbol. A global invariant plus deterministic
ordering forces a **single writer**: one logical thread owns all state — per
-symbol and global — and is the only thing that may mutate it. Inputs from all
processes are assigned a total order at a single ingress and appended to a
persisted, totally-ordered input log; Core's state is a pure fold over that log,
and **replay re-feeds the log through the identical fold with external side
effects suppressed**.

For the MVP this is implemented as a **single-threaded deterministic kernel**
with I/O and persistence offloaded to async worker threads that feed results
back as events — the model proven in production by NautilusTrader. The single
*writer* (the decision stage) is fundamental and stays; the single *thread* is
an MVP implementation detail we can later replace with a staged pipeline
(LMAX-Disruptor-shaped: parallel parse/journal/publish stages around one
decision stage) **if and only if** profiling shows the kernel is the bottleneck.

## Considered options

- *Shard by Symbol* — rejected: global cross-symbol invariants would force a
  distributed two-phase reservation on the hot path, fighting both latency and
  determinism.
- *Single-writer, single-threaded kernel + offloaded async I/O (MVP)* —
  accepted. Matches NautilusTrader's shipping architecture.
- *LMAX-Disruptor staged pipeline (single decision stage)* — the measured
  optimization path, not built until needed. Caveats: stall propagation,
  busy-spin CPU burn, cache-line/`unsafe` tuning, ring sizing, debuggability.

## Consequences

- The persisted ordered input log is the `oath-persistence-core` event log; Core
  is an event-sourced state machine and persistence is its journal.
- **Recovery = snapshot + replay-tail**: periodically snapshot Core state; on
  restart, load the last snapshot then replay only the log tail after it. A
  durable Bus backend (Kafka, Chronicle Queue) can serve as this log (see
  ADR-0002).
- A hard line separates the *deterministic decision* (replayable) from the
  *effectful action* (live-only, suppressed during replay). Reconciling
  in-flight orders with the broker after a crash is a known open problem.
- The MVP kernel is bounded by one core; parallel work (ML, analytics) runs
  off-loop in workers and feeds results back as events.
- `unsafe` is permitted where it demonstrably buys performance, via a justified
  per-crate override of the workspace `unsafe_code = "deny"` lint.

## Future fault tolerance

A **hot-standby Core** that replays the same log via consensus (Aeron-Cluster
style replicated deterministic state machine) is the documented path to Core
failover. Out of scope for the MVP; recorded so the single-writer model is not
mistaken for a single point of failure with no exit.
