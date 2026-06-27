# Environments bind multiple same-safety-class brokers; shadow is targeting, not a tag

_Refines [ADR-0011](0011-execution-environments-mode-isolation.md)._

ADR-0011 isolated trading **modes** into Environments but described each as binding
an execution backend in the **singular**, implying one broker per Environment. That
was an unexamined wording, not a derived requirement. ADR-0011's isolation rationale
is entirely (a) **temporal homogeneity** (one clock profile) and (b) the **cardinal
collision** — a Simulated or Paper [Fill] must never perturb Live risk. Neither
rationale distinguishes two *Live* brokers from each other: they share the same
clock, the same safety-class, and both move real money.

So an Environment occupies **one cell of the mode matrix** — `(temporal profile ×
execution safety-class: Simulated | Paper | Live)` — and within that cell binds
**one or more execution backends of the same safety-class**. Multiple Live brokers
(e.g. IBKR + Coinbase) co-bind into one Core, which makes **cross-broker net
exposure, global per-`InstrumentId` position tracking, and cross-asset budget rules**
(e.g. FX-50 % / equity-50 %) **first-class, in-fold risk** over the canonical
`InstrumentId` — not a derived cross-Environment view. [Position]s remain keyed
`(Account, InstrumentId)` and are **never netted across [Account]s**; the risk fold
aggregates over them.

Cross-cell still always separates: a differing temporal profile or safety-class
forces distinct Environments (Live vs Paper vs Shadow vs Backtest). Same-cell books
may still be split **by choice** for risk isolation; several Live Environments may
run at once.

**Shadow stays a separate Environment** (live feed read-only × Simulated), **not a
per-signal tag in the Live Core.** The Frontend's per-strategy **live / shadow /
off** is realized as strategy-lifecycle **targeting** — register the Strategy-Node
instance against the Live Environment, the Shadow Environment, or neither — over
ADR-0017 operational control + ADR-0013 admission. "Routed to the Simulated Broker"
is therefore **structural** (which Environment the strategy is registered to), never
a conditional the Live Core must evaluate correctly.

## Considered options

- _One broker per Environment_ (ADR-0011 as literally worded) — rejected: it pushes
  cross-broker risk, positions, and budget into a cross-Environment derived view,
  when the isolation rationale never required the split.
- _Shadow as a `SHADOW` tag on signals inside the Live Core_ — rejected: this is
  exactly ADR-0011's already-rejected "environment-tagged messages," lowered to the
  signal layer. One tag-check bug routes a shadow signal to the live account (the
  cardinal collision), and a runaway shadow strategy would share the Live Core's
  fold, Bus, and CPU.
- _Multi-broker Environment + shadow-as-targeting_ (chosen): cross-broker risk in
  one fold; mode/safety isolation preserved **structurally**; promotion shadow → live
  is a deliberate, audited re-registration rather than a toggle.

## Consequences

- **Shared fate is explicit and accepted.** Brokers co-bound into one Environment
  share a Core; a Core fault or [Emergency Halt] affects all of them at once. This is
  *inherent* to unified cross-broker risk — wanting cross-broker netting **is**
  wanting shared fate. Risk isolation remains available by running separate
  Environments.
- **`Account` becomes a first-class key** (new glossary term): a normalized
  composite that includes [Source], with several allowed per [Broker]; Positions are
  keyed `(Account, InstrumentId)`, never netted across Accounts.
- **Canonical `InstrumentId` is promoted to an in-fold risk key**, not merely a Frontend
  join key — which directly shapes the symbology layer (the next thread).
- **Promotion shadow → live is a lifecycle re-registration** (ADR-0013 admission,
  ADR-0017 control), deliberately *not* a one-click flag — going from zero-capital to
  real-money is an explicit audited act.
- **Reconciliation (ADR-0006) runs per Account;** one Environment may run several
  reconciliation loops. The Event Log records every co-bound broker's inputs for the
  book, fused by the event-time merge (ADR-0012).
- Refines ADR-0011; consistent with ADR-0013 (strategy registration) and ADR-0017
  (operational-only control).
