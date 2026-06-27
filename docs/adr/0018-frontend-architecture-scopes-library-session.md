# Frontend architecture: two observation scopes, library/presentation split, session model

The Frontend is factored as a reusable **`frontend-core` library** — Environment
discovery, namespace attach/**switch**, the three-plane consumers (Business State,
Domain Events, Telemetry), and the query/control clients — with **thin
presentations** on top (the `cli` binary first; `tui` / `web` later reuse the same
library, differing only in how they present and switch). It observes through **two
scopes**:

- **Host scope** — an always-on connection to the **Supervisor**: Environment
  discovery, process health, topology, and lifecycle + halt control (ADR-0017).
- **Environment scope** — one Core's **Bus namespace**: that Environment's
  Business State, Domain Events, Environment Telemetry, and queries.

There is **one host Bus** with a namespace per Environment (ADR-0011), so the
Frontend makes one Bus connection and filters by namespace; **switching
Environment is a first-class interaction** (detach namespace A, attach namespace
B — the Supervisor connection persists). The MVP `cli` is a **persistent
interactive session** (live three-plane view; in-session commands including
`env <name>` to switch and `halt`) and *also* exposes **one-shot subcommands** for
scripting — both over the one library. Discovery is **Supervisor-driven** (it
spawned the Environments). MVP attaches to **one Environment at a time**;
multi-Environment side-by-side (e.g. Shadow-vs-Live) is a later additive view, the
capability (subscribe to N namespaces) already present.

## Considered options

- _One-shot CLI only for MVP_ — rejected: a push-spine observability tool that
  cannot hold a live view underuses the telemetry/state design, and the live
  session would be net-new work for the TUI instead of an evolution of the CLI.
  One-shot is kept for scripting, not as the only mode.
- _CLI as a monolith (no shared library)_ — rejected: every later Frontend (TUI,
  web) would re-implement discovery, switching, and the plane consumers. The
  library/presentation split is what the glossary's "CLI is the first Frontend;
  TUIs/web later" implies.
- _Per-Environment Bus instances_ — rejected (already by ADR-0011): one host Bus,
  namespaced, lets the Frontend connect once; a faster-than-real-time Backtest
  namespace cannot drown Live, and the coalescing snapshot (ADR-0015) absorbs the
  flood.
- _Config-file Environment discovery_ — rejected: it duplicates the Supervisor's
  authoritative topology and drifts. The Supervisor spawned them, so it answers
  "what is running."
- _Multi-Environment aggregation in MVP_ — deferred: real value for Shadow-vs-Live,
  but additive; the multi-namespace subscription is available when the combined
  view is built.

## Consequences

- **Switching is cheap and central:** a switch is namespace re-subscription inside
  `frontend-core`; presentations expose it however suits them (typed command,
  dropdown). The host-scope Supervisor connection is stable across switches.
- **Two scopes, two cadences:** host scope (health / topology / discovery) is
  low-rate and always present; Environment scope is the high-rate three-plane
  stream for the attached Environment only — no cross-Environment contamination,
  honoring ADR-0011 isolation.
- **The library is the contract surface:** future Frontends depend on
  `frontend-core` (and through it the public message model + Bus + query
  interface), never on Core internals (CONTEXT: Frontend).
- **`frontend-core` is a small addition to ADR-0009's tree** (a lib crate beside
  `cli`), preserving the dependency direction — it depends only on the public
  message model, Bus, and query interface.
