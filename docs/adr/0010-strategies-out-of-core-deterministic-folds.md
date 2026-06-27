# Strategies as out-of-Core deterministic folds with capability-derived backtest fidelity

Strategies run only in **Strategy Node** processes — never co-located in Core
(superseding ADR-0001's "initially also hosts strategies") — and sit entirely
**outside Core's deterministic boundary**: the Event Log records their Signals as
ordered inputs, and Replay never re-runs a strategy. A Strategy is a
**deterministic fold** over its Bus inputs plus a framework-injected clock and
seeded RNG; external or nondeterministic data must enter as **Data Provider**
messages on the Bus (ADR-0003) — ADR-0005's "deterministic core + offloaded I/O"
applied one level out. Backtest-safety is consequently a **fidelity label, not a
gate**.

## Status

Accepted. Supersedes the strategy co-location in ADR-0001 — strategies are never
in-process with Core.

## Considered options

- _Strategies inside Core's deterministic fold_ (re-run on Replay) — rejected: it
  would forbid the async / ML / external-data strategies that Data Providers exist
  to feed, purchasing only "free" strategy replay.
- _Co-location in Core for the simple case_ (ADR-0001) — rejected and superseded: a
  co-located strategy shares Core's address space and scheduler (a panic or
  hot-loop hits the kernel — the exact fault isolation ADR-0001 exists for), and
  being compile-time-bound it cannot be hot-pluggable, contradicting ADR-0001's own
  goal and ADR-0007 (runtime-pluggable ⟺ separate process).
- _A self-declared `backtest_safe` flag_ — rejected: a claim, not a guarantee; a
  "safe" strategy can still read the OS clock, so the flag's backtests cannot be
  trusted.
- _Capability-derived contexts_ (chosen): a deterministic strategy binds a
  `DetCtx` (injected clock, seeded RNG, `emit`) and **structurally cannot** be
  nondeterministic through the framework; an effectful strategy binds an `IoCtx`
  that adds ad-hoc I/O.

## Consequences

- **Backtest is a fidelity label.** A `DetCtx` strategy's Backtest reproducibly
  matches live; an `IoCtx` strategy still backtests, but parity and freedom from
  **lookahead** are the author's responsibility (the framework cannot rewind a live
  API). Effectful strategies are never forbidden from backtest — only labelled
  advisory.
- **The seam is the Bus even in-process.** A strategy is decoupled from Core's fold
  by construction; the in-memory Bus backend is now a test/backtest device, not a
  production co-location mode.
- **Enforced by construction now, physically later.** `DetCtx`/`IoCtx` is enforced
  because the framework supplies no other inputs — the same discipline as ADR-0008's
  read-only `StateView`. A WASM sandbox that withholds nondeterministic
  host-functions is later hardening that also serves fault isolation (candidate
  Extism; see ADR-0013).
- **Time and timers are always framework-injected, never ambient** — a strategy
  that calls `sleep` or `SystemTime::now()` breaks Replay and Backtest.
