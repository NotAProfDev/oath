# Execution Environments and trading-mode isolation

A trading **mode** is not a concept Core knows; it is the pair _(data feed ×
execution backend)_ the Supervisor wires into an isolated **Environment**. The
**Simulated Broker** is just a Broker-adapter backend (ADR-0003) that fills against
the Environment's own market data, so **Backtest, Shadow, Paper, and Live are cells
of one matrix** and Core + Strategy code are **byte-identical across all of them**.
Each Environment is a fully isolated instance — its own Core, Event Log,
portfolio/risk state, execution-adapter binding, data feeds, and Bus namespace — so
several run on one host without their orders, fills, positions, or logs colliding.

## Considered options

- _One Core, environment-tagged messages_ — rejected: a single bug in one tag check
  spends real money; tagging makes the cardinal collision (a test order reaching the
  live account, a paper fill perturbing live risk) merely unlikely rather than
  structurally impossible.
- _A separate Environment per running mode_ (chosen): the multi-process topology
  (ADR-0001) already gives physical isolation, so a mode is just another
  fault-and-safety domain.

## Consequences

- **The order path and account binding are never shared across Environments.** A
  live and a paper Broker adapter are different processes even for the same venue,
  so a paper order — published only into the paper Bus namespace — physically cannot
  reach the live account.
- **An Environment is temporally homogeneous:** all its feeds share one as-of /
  delay profile (real-time, delayed-by-D, or historical), and event-time processing
  then aligns them (a 15-min-delayed paper feed needs its enrichment delayed to
  match, e.g. via a delay relay). Feeds are shareable read-only only across
  **same-profile** Environments — Live + Shadow-on-live, yes; Live + delayed-Paper,
  no.
- **Shadow** = live feed × Simulated Broker, run alongside Live or Paper to test a
  strategy on real-time data at zero capital risk. Because the sim fills against the
  _exact_ data the strategy saw, Shadow tests _your_ model end-to-end, whereas Paper
  tests the _broker's_ real execution path.
- A **single-Environment** deployment is the trivial N=1 case; the isolation
  machinery is invisible when unused.
- **Replay and Snapshot are per-Environment;** only Live's Event Log is audit-grade.
