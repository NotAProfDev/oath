# `net-ws` resilience surface — design

**Status:** Approved design, pre-implementation.
**Date:** 2026-07-04.
**Crates:**

- `oath-adapter-net-ws-api` (`crates/adapter/net/ws/api`) — the resilience stack lands here
  (reconnect actor, layers, `stack()`), on top of the contract shipped in #65.
- `oath-adapter-net-ws-mock` (`crates/adapter/net/ws/mock`, dev-only) — gains `MockSpawn`.
- `oath-adapter-net-mock` (`crates/adapter/net/mock`, **new, dev-only**) — `MockTimer`, extracted
  from `net-http-mock` to sit beside the transport-neutral `Timer` contract in `net-api`, so both
  the HTTP and WS stacks fake the same clock without dev-depending on each other's crate.
- `oath-adapter-net-ws-tungstenite` (`crates/adapter/net/ws/tungstenite`, **future**) — the real
  backend leaf; **roadmapped here, designed in its own spec** (PR6).

## Context

[ADR-0032](../../adr/0032-websocket-transport-contract-duplex-frames-lifecycle.md) fixed the
`net-ws-api` **contract** — the untyped duplex frame channel, the asymmetric `Stream`-recv /
RPITIT-send split, the epoch-stamped lifecycle channel, the no-silent-drop backpressure
*guarantee*, the `WsConnector` leaf seam, and the per-transport `AuthSource` — and that contract
**already landed** (`Frame`/`CloseFrame`, `WsError`, `WsSink`/`WsSource`, `Lifecycle`/
`LifecycleSnapshot`, `WsConnector`, plus the `net-ws-mock` harness) in **PR #65**.

[ADR-0033](../../adr/0033-websocket-resilience-reconnect-actor-watch-lifecycle.md) then fixed the
**resilience stack that wraps that contract** — the reconnect actor over a new `Spawn` seam, the
two-axis layer stack, the watch-lifecycle *delivery form*, the dual-bound drop-oldest buffer
mechanism, the inverting-but-surviving circuit breaker, the send-axis rate limit, and the
`stack()`/`build()` construction split. **ADR-0033 answers every architectural question.** This
spec does **not** re-decide any of it.

What ADR-0033 deliberately leaves to implementation — and what this spec closes — is the
**decomposition**: how a stack this size is carved into small, one-issue-one-PR slices that are
each independently mock-testable *before* the pieces they compose exist. The governing constraint
(ADR-0033 §9, ADR-0031's rationale) is that the resilience logic lives in the contract crate, not
the backend, precisely so a mock clock + mock spawn + mock leaf can regression-test it; the cut
lines below all serve that testability.

### Governing ADRs

- **ADR-0033** — the WebSocket resilience decision record; the **source of truth** for every
  behavior sliced below. Section references (e.g. "§7") point into it.
- **ADR-0032** — the WS contract this stack wraps; landed in #65. `WsConnector::connect` (the
  composition seam) and the `Lifecycle` watch are unchanged by this spec.
- **ADR-0029** — the runtime-neutral kernel: `Layer`/`ServiceBuilder` (assembly ergonomics only,
  ADR-0033 §1), `ErrorKind`/`HasErrorKind` (breaker classification, §7), the `Timer` clock, and
  the compile-time `impl`/RPITIT-no-`dyn` binding style.
- **ADR-0003 / ADR-0006** — the adapter anti-corruption boundary: session keepalive, subscription
  replay, and loss reconciliation are venue grammar and stay in `oath-adapter-ibkr`, not here.
- **ADR-0004 / ADR-0022** — consume the lifecycle watch (`down_since`/`attempts`/`epoch`) for the
  risk-layer trading halt that the inverting breaker relocates the "break" to (§7).
- **ADR-0014** — the Telemetry plane that the deferred lossy edge-feed and `Tracing` route to.
- **[ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md)** — the HTTP
  construction-surface ADR (landed #66). Two of its decisions bind this spec directly: **§1** is the
  authoritative `AuthSource` shape PR1's WS mirror copies; **§Amendments.4** *mandates* the shared
  `net-mock` extraction (PR0) and names "the WS resilience slice (ADR-0033 §9) is imminent" as the
  time-critical reason — so PR0 is not this spec's invention but an already-recorded ADR decision.
- The **net-http construction-surface spec**
  ([2026-06-30](2026-06-30-net-http-construction-surface-design.md)) — the sibling pattern this
  spec mirrors (decomposition spec + PR map + shared `net-mock` crate).

## Goal

Turn ADR-0033's resilience stack into an ordered PR map in which every slice: (a) lands one
issue → one branch → one worktree → one PR under `just ci`; (b) is fully unit- or table-testable
against `MockWsConnector` + `MockTimer` + `MockSpawn` at the moment it lands, with no "untestable
until a later PR" gap; and (c) keeps each highest-consequence behavior — the invert-vs-survive
breaker above all — in a pure, table-tested unit rather than buried in the un-unit-testable actor
loop. The terminal state of this spec is a plan for **PR0**, then successive plans per PR.

## Scope (in)

The `net-ws-api` resilience surface and its mock infrastructure — **PRs 0–5**:

- **`Spawn`** seam (§2) in `net-ws-api`; `MockSpawn` deterministic executor in `net-ws-mock`.
- **`AuthSource` + `NoAuth`** (§8, ADR-0032 §8) in `net-ws-api` — the WS mirror of the HTTP seam,
  over the `http::Request<()>` handshake.
- **`WsConfig`** (§9) — non-generic plain data; every knob the layers read.
- **The recv-axis units** (§3/§4/§6): `Heartbeat`/liveness and the dual-bound drop-oldest
  `Buffer`.
- **The pure pacing & policy units** (§2/§7/§8): `SendRateLimit` and the extracted
  **`ReconnectPolicy`** (classification + backoff + attempt-rate gate).
- **The reconnect actor** (§2/§5/§7): connect-time `Auth`/`ConnectTimeout`, the spawned actor,
  epoch/lifecycle writes, `ReconnectingConnection` + `WsControl`, `ReconnectingConnector`.
- **`stack()`** (§9) — assembly over an arbitrary leaf — plus `Tracing` (§3) and the
  ordering-invariant regression matrix.
- **The shared `oath-adapter-net-mock` crate (PR0)** — `MockTimer` extracted from `net-http-mock`
  so WS can fake the clock without a dev-dep on an HTTP crate.

## Non-goals (deferred — each its own issue/PR or spec)

| Deferred item | Why deferred | Lands with |
| --- | --- | --- |
| `net-ws-tungstenite` leaf + `build()` (tokio `Spawn`/`Timer`, tokio-tungstenite + rustls, real-socket tests) | Real-backend I/O + integration concerns deserve their own design pass; the `net-ws-api` surface is fully mock-testable without it (ADR-0033 §9) | **PR6 — its own spec** |
| The **lossy edge-transition feed** (§5 last bullet) | Audit/telemetry only (ADR-0014 plane), explicitly *out* of the safety channel so the safety channel carries no never-drop obligation for the audit log; not needed by risk logic, which keys on the watch's epoch | Telemetry integration / adapter, not this workstream |
| Session keepalive (`tic`, `/tickle`), per-topic `smd` staleness timer + `umd+`/`smd+` refresh, subscription replay on `Resumed`, conservative reconcile-on-`Lagged`, venue sequence-gap detection, the `ErrorKind`→permanent classification *refinement* hook values, and the concrete `WsConfig` values | Venue grammar — the ADR-0003/0006 boundary (ADR-0033 Consequences) | The `oath-adapter-ibkr` slices |
| `max_attempts` voluntary give-up on a non-critical stream (§7) | Orthogonal axis to involuntary permanent failure; not in core `ConnState` | If/when a non-critical stream needs it |

## Decisions

### The PR map

All code lands in `net-ws-api` unless noted. Each PR is one issue → one worktree under
`.claude/worktrees/<slug>` → one PR (`Closes #N`), `CHANGELOG.md` `[Unreleased]` updated, `just ci`
green.

| PR | Contents | Crate(s) | New deps | Testable at landing via |
| --- | --- | --- | --- | --- |
| **PR0** | Extract `MockTimer` → new `oath-adapter-net-mock`; repoint `net-http-mock` | `net-mock` (new), `net-http-mock`, `net-http-api` (dev-dep) | — | existing HTTP tests still green off the moved clock |
| **PR1** | `Spawn` seam; `AuthSource`+`NoAuth`; `WsConfig` plain-data; `MockSpawn` | `net-ws-api`, `net-ws-mock` | (minimal) | inline stubs + `NoAuth` + `MockSpawn` step-pump |
| **PR2** | `Heartbeat` (auto-`Pong`, swallow `Pong`, `Close`→lifecycle, idle→`Stale`, active idle-probe); `Buffer` (dual-bound drop-oldest ring, `total_lagged`) — **recv axis** | `net-ws-api` | `event-listener`, `futures-util` | `MockTimer` + scripted frames + recording pong sink |
| **PR3** | `SendRateLimit` (token bucket on a `WsSink`, **send axis**); `ReconnectPolicy` (classify → `Decision`, capped-exp backoff, attempt-rate gate — **clock-free, table-only**, connection axis) | `net-ws-api` | — | pure/table tests + `MockTimer` (rate limit only) |
| **PR4** | connect-time `Auth`+`ConnectTimeout`; the spawned reconnect actor (owns socket, channel-backed sink, epoch bump, `Resumed`, lifecycle writes, per-(re)connect auth); **drives** `ReconnectPolicy`; composes Heartbeat-socket-side-of-Buffer + a smoke assertion; `ReconnectingConnection`+`WsControl`; `ReconnectingConnector` | `net-ws-api` | — | `MockSpawn` + `MockWsConnector` scripted disconnects/`ErrorKind`s + `MockTimer` |
| **PR5** | `stack(leaf,cfg,timer,auth,spawn) -> impl ReconnectingConnector`; `Tracing` (outermost span, folded here); the full ordering-invariant regression matrix | `net-ws-api` | `tracing` | full mock stack (leaf+clock+executor) |
| **PR6** | `net-ws-tungstenite` leaf + `build()` — **roadmapped, own spec** | `net-ws-tungstenite` (new) | tokio, tokio-tungstenite, rustls | its own spec |

`async-watch` is **not** a new dep — it landed with the lifecycle channel in #65
([`lifecycle.rs`](../../../crates/adapter/net/ws/api/src/lifecycle.rs)). Each PR2/PR5 external dep
maps to exactly one named unit (`event-listener`→`Buffer` wakeups; `futures-util`→stream
processing, promoted from #65's dev-dep to a production dep per ADR-0032; `tracing`→the `Tracing`
layer).

### PR0 — the shared `net-mock` crate

**ADR-0034 §Amendments.4 already decided this** — it relocates `MockTimer` into a new dev-only
`oath-adapter-net-mock` (`crates/adapter/net/mock`) beside the `Timer` contract, expressly because
"the WS resilience slice (ADR-0033 §9) is imminent" and the alternative is duplicating `MockTimer`
or dev-depending a *WS* mock on an *HTTP* mock (the nonsense edge across the crate cut). PR0 is that
extraction, executed as the first step of this workstream. It supersedes ADR-0033 §9's original
placement of `MockTimer` in `net-ws-mock`.

Scope (decisive, per ADR-0034): **move** `MockTimer` out — `net-http-mock` keeps **only**
`MockClient` — into `oath-adapter-net-mock`, add the new member + README graph entry, and repoint
the `net-http-api` / `net-http-mock` tests' dev-dep to it. Acceptance: the existing HTTP tests stay
green off the moved clock, **and** the production-reachability guard holds — `cargo tree -e no-dev
-i oath-adapter-net-mock` shows no non-dev dependents (ADR-0034 §Amendments.4; the same guard
`net-http-mock` and `net-ws-mock` carry). `MockSpawn` stays in `net-ws-mock` (PR1) because it mocks
a `net-ws-api` trait, not the transport-neutral `Timer`.

### PR1 — seams + mock infra

- **`Spawn`** (§2): a minimal runtime-neutral seam in `net-ws-api` — the second abstraction
  alongside `Timer`, for the one long-lived task (the actor) that outlives any single `call`. Shape
  (pinned exactly at the TDD step):

  ```rust
  pub trait Spawn: Clone + Send + Sync + 'static {
      /// Spawn a detached long-lived task. Shutdown is via `WsControl::shutdown`
      /// (a channel the task selects on), not an abort handle — so a fire-and-forget
      /// return keeps the seam minimal (ADR-0033 §2).
      fn spawn(&self, task: impl Future<Output = ()> + Send + 'static);
  }
  ```

- **`AuthSource` + `NoAuth`** (§8; ADR-0032 §8): the WS mirror of the **ADR-0034 §1** HTTP seam,
  deliberately the **same seam design** (RPITIT, `Send`-bounded, mutate-in-place, one concrete
  transport error), with the two necessary transport differences: it stamps the **`http::Request<()>`
  handshake parts** (the WS upgrade is a bodyless GET), and its error is **`WsError`**. Unlike HTTP —
  which added `HttpError::Auth` in its PR2 for this seam — **`WsError::Auth` already exists** (landed
  with the #65 contract, `→ ErrorKind::Auth`), so PR1 adds **no** new error variant.

  ```rust
  pub trait AuthSource: Clone + Send + Sync {
      fn authorize(&self, handshake: &mut http::Request<()>)
          -> impl Future<Output = Result<(), WsError>> + Send;
  }
  pub struct NoAuth; // IBKR local gateway holds the session cookie → ready Ok(())
  ```

  Landed in PR1 as a foundational contract, tested via `NoAuth` (ready-`Ok`, `Send`-assertion, like
  HTTP's); **first consumed in PR4** (connect-time `Auth`, re-stamped per (re)connect).

- **`WsConfig`** (§9): non-generic plain data — connect timeout; backoff (base, cap, factor) +
  connection-attempt-rate cap (max attempts / window); buffer bounds (`max_count`, `max_bytes`);
  liveness (idle-read timeout, active-ping interval, idle threshold); send-rate (tokens, refill);
  permanent-error policy (retries-before-`Unrecoverable`). **No `RateKey`/`K` generic and no
  boot-time coverage check** — a WS send limit is per-connection (one pipe), a genuine reduction
  vs. the net-http surface (§9). Landed whole; later PRs read subsets.

- **`MockSpawn`** (net-ws-mock, §9): a test-controlled, single-threaded, manually-pumped executor —
  *not* a tokio spawner. The whole point of the `Spawn` seam: only a deterministic executor lets a
  test drive the actor step by step and assert invariants without racing a background task
  (`Timer`-style "controllable, not a no-op" applied to spawning).

### PR2 — recv-axis units

Built as **independent, standalone units with their own tests** (not buried in the actor loop, per
the deep-module cut that also governs the send/policy units) so both land and are fully tested
*before* the PR4 actor that composes them exists.

- **`Heartbeat`** (§4): a frame-stream processor that, given the socket source, a `Pong`-capable
  sink handle, a `Timer`, a `LifecycleSender`, and the `WsConfig` liveness knobs, yields a
  **data-only** frame stream: auto-`Pong` on `Ping` (**mandatory** — Binance drops a socket that
  misses it), swallow `Pong`, map `Close` to a lifecycle transition, a passive idle-read timeout →
  `Stale`, and an active protocol-`Ping` when idle (*keepalive-when-idle*, since IBKR guarantees no
  heartbeat on an idle/unsubscribed socket). It handles **transport liveness only**; session
  keepalive is the adapter's (the hard §4 split). Table-tested with `MockTimer` + scripted frames +
  a recording pong sink.
- **`Buffer`** (§6): the dual-bound drop-oldest ring — a producer (push data frame; on
  `min(count, bytes)` overflow evict oldest, increment `total_lagged`; **never** drop the newest,
  so a lone frame larger than the whole byte budget is still admitted) and a consumer (`Stream` of
  `Frame`, `event-listener` wakeups — an `mpsc` cannot drop-oldest). Byte-accounting defeats the
  IBKR-small-JSON assumption that OOMs on a Coinbase multi-MB level2 snapshot. A standalone data
  structure; table-tested for eviction order, both bounds, oversized-frame admission, and the
  `total_lagged` count.

### PR3 — pure pacing & policy units

The two guard-rail units, grouped because both are pure and table-testable with no socket, spawn,
or actor. (If you later want `ReconnectPolicy` on its own slice, it splits cleanly — nothing in
PR3 couples the two.)

- **`SendRateLimit`** (§8): a token bucket wrapping a `WsSink`; `send()` awaits a token
  (`Timer`-driven refill) — *backpressure-inside-`call`*, consistent with ADR-0032 §2 (which
  rejected `Sink`'s `poll_ready` handshake, not all backpressure). Default off/generous so IBKR
  never notices; configured with the venue inbound cap so a reconnect resubscribe-burst (~100 lines
  > Binance's ~5/s) cannot flood → disconnect → ban. Table-tested with `MockTimer`.
- **`ReconnectPolicy`** (§2/§7) — the **highest-consequence logic in the stack, kept out of the
  actor loop.** Three pure mechanisms behind one unit:
  - `classify(kind: ErrorKind, attempts: u64, /* adapter hook */) -> Decision` where
    `Decision::{ RetryAfter(Duration), Unrecoverable }`. Transient/unknown (`Connection`/`Timeout`)
    → `RetryAfter` **forever** (the "break" relocates to the risk-layer halt via the watch, §7);
    permanent (`Auth`, protocol reject) → a few retries then `Unrecoverable` (retrying a permanent
    failure worsens the outage → Binance ban).
  - capped-exponential `backoff(attempts) -> Duration`.
  - the connection-attempt-rate gate: `admit(history, now: Instant) -> Result<(), Duration>` (300
    conns / 5 min / IP) so reconnect itself cannot storm into a ban.
  **Clock-free**: `now` is an *input*; the actor owns the `Timer` and feeds it. Pure table tests
  cover the full invert-vs-survive truth table with zero async.

### PR4 — the reconnect actor (the heart)

- **connect-time `Auth`** (wraps a `WsConnector`, stamps the handshake via `AuthSource` before
  `leaf.connect`, re-run per attempt) and **`ConnectTimeout`** (a fresh `Timer`-bounded timeout per
  attempt, so a hung handshake cannot wedge the backoff loop — §3's *ConnectTimeout-inside-Reconnect*).
- **The spawned actor** (§2/§5): owns the single socket; drains a **channel-backed** stable `WsSink`
  (the adapter's sends survive a reconnect); on a break, consults `ReconnectPolicy`, rebuilds via
  `leaf.connect`, re-injects auth, **bumps the epoch**, emits `Resumed{epoch}`, and writes each
  `LifecycleSnapshot` through the `LifecycleSender` (overwrite, never blocks — the actor is never
  backpressured by a slow risk consumer). It **drives** `ReconnectPolicy` (it is no longer *the*
  policy) and composes **Heartbeat socket-side of Buffer** (control frames handled before the data
  ring; auto-`Pong` never queued behind a slow data consumer).
- **`ReconnectingConnection { sink, source, lifecycle, control }`** + **`WsControl { force_reconnect,
  shutdown }`** — the *usage* seam (§1), the richer handle produced only at assembly and handed to
  the adapter once; the control verbs live **here, not on the sink** (data/control-plane split).
  **`ReconnectingConnector`** is the factory trait `stack()`/`build()` return.
- **Ordering smoke assertion:** even though the full invariant matrix is a `stack()` property
  (untestable until PR5, §9), PR4 ships at least one behavioral test that the actor it wires
  absorbs a control frame (sends `Pong`) and that frame **never** surfaces in the buffered source —
  so no one-PR window ships an actor with an unasserted recv order.

### PR5 — assembly, invariants, and `Tracing`

- **`stack<S, T, A, Sp>(leaf, cfg, timer, auth, spawn) -> impl ReconnectingConnector`** (§9): the
  canonical assembly over an arbitrary leaf, so the ordering invariants are regression-testable over
  a deterministic leaf + clock + executor.
- **`Tracing`** (§3): the outermost span over all reconnects (secret-safe — auth is injected below
  it). **Folded here, not given a lonely PR2 test**: a span wrapper's only real contract is
  *outermost placement*, which is a `stack()` invariant, and asserting span entry needs a
  subscriber — set up once, here, where the assembly tests live.
- **The ordering-invariant matrix** over `MockWsConnector` + `MockTimer` + `MockSpawn` (§3): `Auth`
  innermost re-stamped per (re)connect (`Reconnect` = the `Retry`-analogue); `ConnectTimeout` inside
  `Reconnect`; `Heartbeat` socket-side of `Buffer`; `Tracing` outermost. Plus the cross-cutting
  behaviors the full stack now makes testable end-to-end: transient→retry-forever + `down_since`/
  `attempts` climb; permanent→`Unrecoverable`; epoch bump + `Resumed` on reconnect; auto-`Pong`
  below a full buffer; `force_reconnect` via the control handle.

## Resolved implementation open questions

1. **Slice boundaries** → the PR0–5 map above (fine-grained, mirroring HTTP Slice-0's cadence);
   PR6 (leaf) roadmapped to its own spec.
2. **`ReconnectPolicy` extraction** → its own pure unit in PR3 (classification + backoff +
   attempt-rate gate), clock-free, table-only — the invert-vs-survive breaker does not live in the
   actor loop.
3. **`Tracing` placement** → PR5 (assembly), not a contentless PR2 unit.
4. **`MockTimer` home** → shared `oath-adapter-net-mock` (PR0), extracted from `net-http-mock` per
   **ADR-0034 §Amendments.4** (which already recorded this relocation, superseding ADR-0033 §9);
   `MockSpawn` stays in `net-ws-mock` (it mocks a `net-ws-api` trait). Both mocks keep the
   production-reachability guard.
5. **`AuthSource` placement & shape** → declared in PR1 (foundational, tested via `NoAuth`),
   consumed in PR4; the HTTP seam's mirror over `http::Request<()>` / `WsError`.
6. **Lossy edge feed (§5)** → explicitly deferred (ADR-0014 telemetry plane), a decision not a
   silent drop.
7. **Actor ordering coverage** → PR4 ships a recv-order smoke assertion; the full matrix is PR5.

## Consequences

- **New crate `oath-adapter-net-mock`** (dev-only) holding the shared `MockTimer` per ADR-0034
  §Amendments.4; `net-http-mock` keeps only `MockClient`; `net-ws-mock` gains `MockSpawn`. Both
  mock crates remain dev-only — verified by the `cargo tree -e no-dev -i` reachability guard.
- **`net-ws-api` gains** (over #65): `Spawn`, `AuthSource`/`NoAuth`, `WsConfig`, `Heartbeat`,
  `Buffer`, `SendRateLimit`, `ReconnectPolicy`, the reconnect actor, `Auth`/`ConnectTimeout`,
  `ReconnectingConnection`/`ReconnectingConnector`/`WsControl`, `Tracing`, and `stack()`. New
  production deps: `event-listener`, `tracing`, and `futures-util` (promoted from dev). Still
  zero-runtime, zero-I/O — no `tokio`/`tokio-tungstenite`/`rustls`.
- **The adapter (`oath-adapter-ibkr`) owns** (unchanged from ADR-0033 Consequences): session
  keepalive, the `smd` staleness timer + resubscribe, subscription replay on `Resumed`, the
  conservative reconcile-on-`Lagged`, sequence-gap detection, the permanent-classification
  refinement, and the concrete `WsConfig` values.
- **Diverges from ADR-0033 §9** in one respect only — `MockTimer` home → shared `net-mock` — and
  that divergence is not this spec's: it was already recorded by **ADR-0034 §Amendments.4**, which
  this spec follows. Every other ADR-0033 decision is implemented as written.
- **The `net-ws-tungstenite` leaf (PR6)** is the one piece this workstream does not build; the
  `net-ws-api` surface it plugs into is complete and mock-verified after PR5.
</content>
</invoke>
