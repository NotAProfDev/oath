# WebSocket resilience: reconnect actor over a `Spawn` seam, two-axis layer stack, watch-lifecycle, and a circuit breaker that inverts for transient loss but survives for permanent failure

[ADR-0032](0032-websocket-transport-contract-duplex-frames-lifecycle.md) fixed the
**`oath-adapter-net-ws-api` contract** — the untyped duplex frame channel, the asymmetric
`Stream` recv / RPITIT send split, the epoch-stamped lifecycle channel, the uniform
no-silent-drop backpressure *guarantee*, the `WsConnector` leaf seam, and the per-transport
`AuthSource`. It deferred the **resilience stack that wraps that contract** — reconnect,
heartbeat, the §6 buffer *mechanism*, and the default layer order — to this ADR. This is the
WebSocket analogue of [ADR-0031](0031-http-resilience-venue-pacing.md) (the HTTP sibling),
driven by the first [Broker](../../CONTEXT.md)/[Data Provider](../../CONTEXT.md), IBKR's
Client Portal WebSocket, and — because this crate is one of the ADR-0029 series' *generic*,
reusable transports — **cross-checked against Binance and Coinbase**, whose keepalive and
loss models differ from IBKR's in ways that would otherwise leak an IBKR-shaped assumption
into a shared crate.

Every timing layer is generic over
[`net-api::Timer`](0029-network-adapter-stack-transport-split-compile-time-composition.md);
unlike the HTTP stack, the WS reconnect supervisor is a long-lived task, so this ADR adds a
second runtime-neutral seam, `Spawn`, alongside `Timer`.

## The grounding cases

The transport is **grammar-blind** (ADR-0032 §1): it cannot tell a market-data frame from an
order frame, nor a subscription from a keepalive. That single fact forces the split that
recurs throughout this ADR — **transport liveness** (is the socket alive?) is generic and
lives in the layers; **session keepalive** and **loss recovery** (is the *venue* about to
idle-drop us? which *stream* lost data?) are venue grammar and live in the adapter.

| Concern | IBKR CP | Binance spot | Coinbase Advanced Trade |
|---|---|---|---|
| Server keepalive | app `{"topic":"system","hb":…}` Text; **not guaranteed on an idle/unsubscribed socket** | **server sends a protocol `Ping` every 20s**; no `Pong` within 60s → dropped | app `heartbeats` channel (1/s); channels close in 60–90s idle |
| Client keepalive | `tic` Text ~q10s + REST `/tickle` (5-min session) | auto-`Pong`; user streams keep `listenKey` alive via REST | subscribe `heartbeats`; **JWT expires 2 min** → re-auth |
| Inbound send limit | low volume | **~5 msgs/s** (ping/pong/subscribe all count) → disconnect; *"IPs repeatedly disconnected may be banned"*; 300 conns / 5 min / IP | disconnected if no subscribe within 5s |
| Silent partial loss | `smd` self-terminates ~15 min: **ticks for a `conid` stop, socket/session/`hb` all healthy** | listenKey expiry stops the user stream, socket alive | channel idle-close; `sequence`/`heartbeat_counter` gaps |
| Large frames | small JSON | depth diffs small | **level2 BTC-USD snapshot overflows a 100 KB buffer** (clients raise to ~100 MiB) |

These are reference data for the adapters, not domain terms. Two facts do real work below:
Binance proves **auto-`Pong` is mandatory and inbound sends must be paced**; Coinbase proves a
**generic transport can receive multi-MB frames**, so a frame-count-only buffer bound is an
IBKR-shaped assumption that OOMs on venue #2.

## Decision

### 1. Composition: a uniform `WsConnector` *inside*, a richer `ReconnectingConnection` *out*

The kernel's `Layer`/`ServiceBuilder` (ADR-0029 §3, deliberately `Service`-bound-free) is
reused to *assemble* the stack — but that reuse is **assembly ergonomics, not the abstraction
doing the resilience work.** Industry hand-assembles the frame half of a resilient socket; it
does not model reconnect/heartbeat/buffer as a generic middleware stack. What is load-bearing
is the seam split, and there are **two** seams, which ADR-0032's single `WsConnector` conflated:

- **Composition seam** — `WsConnector::connect(handshake) -> (WsSink, WsSource, Lifecycle)`,
  the ADR-0032 §2 triple. Internal; the leaf and every inner layer implement it. This is how
  layers stack; it never reaches an adapter.
- **Usage seam** — a new type `ReconnectingConnection { sink, source, lifecycle, control }`,
  produced only at the assembly boundary and handed to the adapter exactly once. The control
  handle (`WsControl`: `force_reconnect`, `shutdown`) exists **only here**.

```rust
trait WsConnector {                                  // composition — ADR-0032 §2/§4 unchanged
    fn connect(&self, h: http::Request<()>)
        -> impl Future<Output = Result<(WsSink, WsSource, Lifecycle), WsError>> + Send;
}

struct ReconnectingConnection {                      // usage — new here; what a ReconnectingConnector yields
    sink: WsSink,           // {send, close} — minimal, as landed
    source: WsSource,       // Stream<Result<Frame, WsError>>
    lifecycle: Lifecycle,   // last-value watch of LifecycleSnapshot (§5)
    control: WsControl,     // force_reconnect(), shutdown()
}

trait ReconnectingConnector {                        // usage seam — the factory `stack()`/`build()` return (§9);
    fn connect(&self, h: http::Request<()>)          //   the assembled-stack analogue of the leaf `WsConnector`
        -> impl Future<Output = Result<ReconnectingConnection, WsError>> + Send;
}                                                    // (-or = the factory trait, -ion = its product struct)
```

The reconnect layer is the exact **`Layer → Service` analogue** from tower: you compose
`Layer`s but *hold* a `Service` (`ServiceBuilder` yields a `Buffer<Retry<…>>` used as a
`Service`, never as a `Layer`). Composition unit ≠ product type. So the leaf never grows a
control handle it cannot honour — `force_reconnect` on a raw socket would be a silent no-op,
the class of dishonest seam this crate's charter forbids. An adapter that genuinely wants a
raw connection opts in with an explicit `PassthroughReconnect` layer supplying a trivial
`WsControl` in **one** quarantined place, not a triviality forced onto every leaf. This is the
managed-handle pattern industry ships (gRPC's `ManagedChannel` carries `resetConnectBackoff()`
/ `shutdown()`; the raw transport carries none; likewise `ezsockets::Client` vs. a raw
`WebSocketStream`).

### 2. Reconnect is a spawned actor over a runtime-neutral `Spawn` seam

The reconnect supervisor owns the single socket, drains a **channel-backed** `WsSink`, forwards
to the live connection, and on a break rebuilds it, re-injects auth, bumps the epoch, and emits
`Resumed{epoch}`. It is a long-lived task that outlives any single send or poll and coordinates
the two independently-owned halves (ADR-0032 §2) — so, unlike every HTTP layer (which is purely
caller-driven inside `call`), it must **spawn**.

`net-ws-api` is zero-runtime (ADR-0032 Consequences). The reconciliation is the same one
ADR-0029 §4 made for `Timer` and the net-http construction surface made for `async-lock`: a
**`Spawn` trait is an abstraction, not a runtime.** So this ADR declares a minimal `Spawn`
seam in `net-ws-api`; the backend (`net-ws-tungstenite`) provides the tokio impl. The actor
lives in `net-ws-api`, keeping the whole resilience stack — as in ADR-0031 — in the contract
crate, mock-testable and backend-reusable.

- **Backoff** is capped exponential (§7), and also honours a
  **connection-attempt rate** (Binance: 300 conns / 5 min / IP) so reconnect itself cannot
  storm into a ban.
- **Auth is re-pulled per (re)connect** (ADR-0032 §8), never cached at first connect, so a
  session refreshed by `/tickle` between drop and reconnect is picked up — the streaming
  analogue of ADR-0031 §1's per-attempt re-stamp.

### 3. The default stack: two axes, not one line

ADR-0031's single line (`Tracing → CircuitBreaker → Retry → RateLimit → Timeout →
BufferOrStream → Auth → leaf`) does not transliterate, because WS concerns split across two
axes rather than one per-request `call`. (First `.layer()` is outermost — ADR-0029's
`ServiceBuilder` invariant.)

```text
Connect-time (per (re)connect):  Tracing → Reconnect(backoff, epoch) → ConnectTimeout → Auth → leaf.connect
Recv per-frame (socket→adapter): socket → Heartbeat(auto-Pong, absorb control, idle→Stale) → Buffer(drop-oldest, Lagged) → WsSource
Send per-frame (adapter→socket): WsSink{send, close} → SendRateLimit(token bucket) → socket
Control plane:                   WsControl.force_reconnect() → actor
```

The ordering **invariants** (the reason assembly lives once, over an arbitrary leaf, so a
`stack(MockWsConnector, …)` can regression-test them — mirroring ADR-0031's rationale):

- **`Auth` innermost at connect-time, re-stamped per (re)connect** — the WS analogue of
  ADR-0031's *Auth-inside-Retry*: **`Reconnect` is the `Retry`-analogue**, retrying the
  *connection*, with `Auth` re-stamping inside each attempt.
- **`ConnectTimeout` inside `Reconnect`** — each attempt gets a fresh timeout; a hung handshake
  cannot wedge the backoff loop (ADR-0031's *Retry-outside-Timeout*).
- **`Heartbeat` socket-side of `Buffer`** — control frames are handled *before* the data ring
  (ADR-0032 §3/§6), so auto-`Pong` is never queued behind a slow data consumer.
- **`Tracing` outermost** — one span over all reconnects (ADR-0031 §6), a Telemetry source
  (ADR-0014), secret-safe (auth material is injected below it).

### 4. Heartbeat/liveness: transport liveness in the layer, session keepalive in the adapter

The layer is grammar-blind, so it can only ever send a **protocol `Ping`** — which probes the
*socket* but does not satisfy any venue's *session* keepalive (`tic`, `heartbeats`-subscribe,
`listenKey` PUT are all venue grammar). The split is therefore hard:

- **Layer (generic) owns transport liveness:** auto-`Pong` on protocol `Ping` — **mandatory**
  (Binance drops a connection that misses it); swallow `Pong`; map `Close` to a lifecycle
  transition; a **passive idle-read timeout** (`Timer`-driven) → `Stale`; and an **active
  protocol-`Ping` probe when idle** (*keepalive-when-idle*, not flat-off — IBKR gives no
  guaranteed heartbeat on an idle/unsubscribed socket, so a purely passive detector could
  starve on a fresh idle connect). The active probe is a config knob (interval + idle
  threshold).
- **Adapter owns session keepalive** (venue grammar): IBKR `tic`; Coinbase `heartbeats` +
  per-message JWT; Binance `listenKey` — the ADR-0003 boundary carried into liveness.

### 5. Lifecycle: a `watch` of an epoch-stamped snapshot, not a transition stream

ADR-0032 §4 deferred the delivery form. It is resolved here as a **last-value `watch` of a rich
snapshot**, *not* a transition stream and *not* a naive watch of bare `ConnState`:

```rust
struct LifecycleSnapshot {
    phase: ConnState,     // level: Connected/Stale/Reconnecting/Resumed/Unrecoverable
    epoch: u64,           // monotonic: bumped on every completed down-cycle. Canonical source of
                          //   truth — the value echoed in Connected{epoch}/Resumed{epoch} is this
                          //   field; consumers diff it.
    down_since: Option<Instant>,
    attempts: u64,        // monotonic
    total_lagged: u64,    // monotonic cumulative — NOT a per-event delta (see §6)
}
```

- A **transition stream is rejected**: its emitter is the socket-owning actor (§2); a bounded
  stream with a blocking sender couples the actor's liveness to a slow risk consumer — the
  actor stalls, stops answering `Ping`, and *causes* the disconnect it is trying to report —
  worst exactly under the stress that generates down-edges. Unbounded trades that for unbounded
  memory; drop-on-full reintroduces the lost edge.
- A **naive watch of bare `ConnState` is also rejected**: a fast `Stale → Reconnecting →
  Resumed` coalesces to `Resumed`, hiding the safety-critical feed-*down* edge (ADR-0032 §4)
  from a slow risk loop.
- The **epoch resolves both**. The risk loop `select!`s `changed()`, and on wake `borrow()`s
  and diffs `epoch`. Losslessness is by construction: **{ currently-down (`phase`) ∨
  epoch-advanced }** is total — either the consumer reads an in-progress down phase, or a
  fully-coalesced cycle is recovered from the epoch delta ("epoch jumped 5→9 ⇒ four down-cycles,
  regardless of what I witnessed"). Only transient edge *ordering/timing* is lost, which is
  telemetry, not risk logic; even a cancel-all-on-down action keys on epoch-advance, so it is
  idempotent and lossless. The `watch` sender **never blocks** (overwrite semantics), so the
  actor is never backpressured. With the epoch in the level, the watch is the **safe and cheap**
  choice and the transition stream is the one whose safety degrades under load — the inverse of
  the naive framing. (Prior art: `epoll` level-triggered mode, TCP sequence numbers, sticky
  hardware fault registers — level + monotonic version deliver a must-not-miss fact to a slow
  consumer without blocking the producer.)
- **Discipline:** every snapshot field must be **level or monotonic-cumulative, never a
  per-event delta**, or overwrite loses it — which is exactly why `Lagged`'s per-event `count`
  becomes the cumulative `total_lagged` the consumer diffs.
- The `watch` primitive is **runtime-neutral** (`async-watch`, extracted from
  `tokio::sync::watch`, `event-listener` family) — *not* `tokio::sync::watch`, keeping tokio out
  of `net-ws-api`'s graph exactly as the net-http surface chose `async-lock` over `tokio::sync`.
- An **explicitly lossy edge feed** off the actor serves audit/telemetry consumers that want the
  ordered transition trail (the ADR-0014 Telemetry plane), kept *out* of the safety channel so
  the safety channel carries no never-drop obligation for the audit log's sake.

### 6. Backpressure: the §6 buffer mechanism — a dual-bound drop-oldest ring

ADR-0032 §6 fixed the *guarantee* (never silently discard; drop **oldest data** on overflow;
emit `Lagged`; control bypasses; per-stream policy adapter-side). The mechanism:

- **Control-bypass is structural.** The actor's read loop handles `Ping`/`Pong`/`Close`/liveness
  inline and pushes only `Text`/`Binary` into the ring; it **always drains the socket** (§6
  rejects TCP-backpressure) and absorbs pressure on the data side by dropping, never by refusing
  to read. Source wakeups use `event-listener`; an `mpsc` cannot drop-oldest.
- **Dual bound — a soft `min(count, bytes)` *backlog* budget, not a hard memory cap.** A
  frame-count-only bound bakes in IBKR's small-JSON assumption; a generic transport receives
  multi-MB frames (Coinbase level2 snapshot), so `N × frame_size` OOMs on venue #2. Byte-accounting
  is one `usize` (`frame.len()` is already in hand), so the ring evicts oldest frames once *either*
  the count *or* the accumulated bytes trips its bound, whichever comes first; the byte default is
  generous (a few MB, per-venue tunable) so IBKR never touches it. The bound governs the *backlog*,
  not a single in-flight frame: because the newest is never dropped (§6), a lone frame larger than
  the whole byte budget is still **admitted** (older evicted, lag incremented) — so the effective
  peak is `budget + one max frame`, a soft ceiling, not a strict one. This is the standard
  slow-consumer shape (Redis `client-output-buffer-limit`, Kafka `buffer.memory`, Netty
  `WriteBufferWaterMark`).
- **`Lagged` is a blunt, grammar-blind instrument — recorded as a consequence, not a gap.** A
  single global `total_lagged` cannot attribute drops to a stream (per-stream rings would need
  demux = venue grammar in the transport, the forbidden leak; and the dropped frames are gone).
  So any increment forces the adapter to the **conservative union**: if any order stream is live,
  reconcile orders *and* resnapshot MD. Where a venue carries sequence numbers (Coinbase
  `heartbeat_counter`, Binance depth `U`/`u`), the *adapter* gets precise per-stream loss from
  its own sequence tracking after demux; `total_lagged` is only a coarse "something dropped" hint.
  ADR-0032 §6's "reconcile-on-`Lagged` (order) / drop-to-latest (MD)" must not be read as the
  transport distinguishing the two.

### 7. The circuit breaker inverts for transient loss and survives for permanent failure

ADR-0031's `CircuitBreaker` *stops* calling a failing venue. For a market/order feed, going dark
is the emergency (ADR-0004 risk is blindest when the feed is down), so the breaker **inverts**
for transient loss — but only for transient loss:

- **Transient / unknown** (`ErrorKind::Connection`/`Timeout`) → **retry forever, capped backoff.**
  The "break" relocates to a **risk-layer trading halt** ([ADR-0022](0022-reliable-order-path-graduated-failure.md),
  fed by the watch's `down_since`/`attempts`), not a transport give-up — the market-data /
  OTP-supervisor standard: *the transport never stops trying to see; Core decides when to stop
  acting.*
- **Permanent** (`ErrorKind::Auth`, protocol-version rejection) → a few retries (a session expiry
  may re-auth away), then a **terminal `Unrecoverable` phase** — stop. Retrying a permanent
  failure forever cannot succeed, and **worsens** the outage: *"IPs repeatedly disconnected may be
  banned"* is ADR-0031's original "stop hammering a failing dependency" reappearing, and a ban
  takes down healthy connections too. `Unrecoverable` also disambiguates two operational states a
  climbing counter conflates — "a human must rotate the key" vs. "the network is flaky."

Classification is by `ErrorKind` (grammar-free; `WsError: HasErrorKind`, ADR-0032 Consequences),
adapter-refinable via a hook (venue grammar: which close-code is permanent, à la gRPC
`UNAUTHENTICATED` vs. `UNAVAILABLE`). An optional `max_attempts` cap stays **orthogonal** — a
voluntary give-up on a *non-critical* stream, a different axis from involuntary permanent failure.
This ADR does not add it to the core `ConnState` (ADR-0032 §4): if enabled, it surfaces as its own
terminal outcome, distinct from `Unrecoverable`.

### 8. Send-axis `RateLimit`, the control handle, and expiry ≠ death

- **`SendRateLimit`** restores ADR-0031's *proactive* guard on the axis where sends happen: a
  generic token bucket on `WsSink`, adapter-configured with the venue's inbound limit, **default
  off/generous** so IBKR never notices. Without it, a reconnect resubscribe-burst (~100 lines >
  Binance's ~5/s) floods → disconnect → reconnect → flood — a storm reconnect-backoff cannot stop
  (backoff paces *connects*, not *sends*). `send()` awaiting a token is *backpressure-inside-`call`*,
  consistent with ADR-0032 §2 (which rejected `Sink`'s `poll_ready` *handshake*, not all
  backpressure). This pairs with `Reconnect` exactly as ADR-0031's `RateLimit` (never hit the
  limit) pairs with `CircuitBreaker` (recover if you do).
- **`force_reconnect` is a control-handle verb, not a sink method** — keeping the data plane
  (`sink.send`) and control plane separate, the same discipline the lifecycle read-channel
  follows. It serves the *proactive* reconnect the venues demand (Binance 24h connection lifetime,
  Coinbase JWT 2-min re-auth) as well as the reactive one.
- **Expiry ≠ death (adapter, verified).** IBKR `smd` self-termination is a **silent per-`conid`
  death on a healthy socket** — every transport liveness detector stays green. So `force_reconnect`
  is the *wrong* remedy (it would tear down ~100 healthy subscriptions); the adapter owns a
  per-topic staleness timer → `umd+`/`smd+` **resubscribe**, and on `Resumed{epoch}` the adapter
  replays subscriptions (ADR-0032 §5). Reconnect is escalated **only** when the adapter concludes
  the *socket* is dead.

### 9. Construction and mock-testability

Mirroring the net-http `stack()`/`build()` split, for the same reason (the ordering invariants of
§3 are testable only over a deterministic leaf and clock):

```rust
// net-ws-api — assembles the canonical stack over ANY leaf
pub fn stack<S, T, A, Sp>(leaf: S, cfg: WsConfig, timer: T, auth: A, spawn: Sp)
    -> impl ReconnectingConnector
where S: WsConnector + …, T: Timer, A: AuthSource, Sp: Spawn;

// net-ws-tungstenite — builds the tungstenite leaf, then delegates to stack()
pub fn build<T, A, Sp>(cfg: WsConfig, timer: T, auth: A, spawn: Sp, conn: ConnConfig)
    -> impl ReconnectingConnector;
```

- **`WsConfig` is non-generic plain data** (timeouts, backoff + attempt-rate cap, buffer bounds
  count+bytes, heartbeat/idle intervals, send-rate limit, permanent-error policy). **No `K`-generic**
  — a WS send-limit is per-connection (one pipe), not per-endpoint-keyed, so there is no `RateKey`
  and no boot-time coverage check (a genuine reduction versus the net-http surface).
- **New dev-only crate `oath-adapter-net-ws-mock`** (mirror of `net-http-mock`): `MockWsConnector`
  (scripted frames + injectable disconnects and `ErrorKind`s), `MockTimer`, and **`MockSpawn` — a
  test-controlled, single-threaded, manually-pumped executor**, not a tokio spawner. This is the
  point of the `Spawn` seam: only a deterministic executor lets a test drive the actor step by step
  and assert the invariants ("Auth re-stamps inside each reconnect," "auto-`Pong` below a full
  buffer," "permanent vs. transient classification") without racing a background task — the
  `Timer`-style "controllable, not a no-op" discipline applied to spawning.

## Considered options

- *Reconnect in the backend, not `net-ws-api`* — rejected: it removes the mock-clock/mock-spawn
  testability that justifies the whole seam, and a second backend would rewrite the actor. The
  `Spawn` seam keeps the resilience logic in the contract crate (as ADR-0031 keeps HTTP's).
- *Poll-driven reconnect (no spawn, `unfold` + shared cell)* — tenable, but the channel-backed
  actor cleanly owns both halves (auto-`Pong` needs the sink from the recv path) and matches the
  industry actor model; a shared-cell design gets gnarly once heartbeat-`Pong` and send-during-gap
  are in play.
- *Control verbs on the sink, or a 4-tuple `connect()`* — rejected: the sink is the data plane;
  and a 4-tuple would amend ADR-0032 §2's arity and force a meaningless control handle onto every
  raw leaf. The usage type ≠ composition type (tower `Service` vs. `Layer`), so the richer handle
  belongs only at the assembly boundary.
- *Transition-stream lifecycle, or naive watch of bare `ConnState`* — rejected (§5): the first
  couples actor liveness to a slow consumer; the second coalesces away the feed-down edge. Watch of
  an epoch-stamped snapshot is lossless for the safety fact and never blocks the actor.
- *Frame-count-only buffer bound* — rejected (§6): OOMs on a multi-MB-frame venue that the crate's
  generality already includes.
- *Retry every failure forever, or give up after a cap* — rejected (§7): the first hammers a
  permanent failure into a ban; the second blinds a critical feed on a transient hiccup. Classify:
  retry transient forever, surface permanent as `Unrecoverable`.
- *No transport send limit* — rejected (§8): a resubscribe burst floods a venue with an inbound
  cap into a disconnect/ban that reconnect-backoff cannot prevent.

## Consequences

- **New seam:** `Spawn` in `net-ws-api` (runtime-neutral, mirrors `Timer`); the backend supplies
  the tokio impl. **New dep:** `async-watch` (runtime-neutral last-value channel, `event-listener`
  family) on top of ADR-0032's set; still zero-runtime, zero-I/O. **New dev-only crate**
  `oath-adapter-net-ws-mock` (`MockWsConnector`/`MockTimer`/`MockSpawn`, consumed only via
  `[dev-dependencies]`, mirroring the net-http-mock production-reachability discipline).
- **`net-ws-api` gains** the reconnect actor, heartbeat/liveness, the dual-bound drop-oldest
  buffer, `SendRateLimit`, `Tracing`, the `stack()` assembler, `ReconnectingConnection` +
  `WsControl`, and `LifecycleSnapshot`; `WsConfig` (non-generic). `net-ws-tungstenite` owns `build()`
  and the tokio `Spawn`/`Timer`.
- **The adapter (`oath-adapter-ibkr`) owns:** session keepalive (`tic`, `/tickle`), the per-topic
  `smd` staleness timer + `umd+`/`smd+` refresh, subscription replay on `Resumed`, the
  conservative reconcile-on-`Lagged`, sequence-gap detection where the venue offers it, the
  `ErrorKind`→permanent classification refinement, and the `SendRateLimit`/backoff/timeout config
  values.
- **Amends ADR-0032 (in place, same PR — not landed):** §"grounding case" (`smd` ~15 min, silent
  per-topic expiry); §4 (`Unrecoverable` added to `ConnState`; the deferred watch-vs-stream
  sub-choice resolved as a `watch` of `LifecycleSnapshot`; `Lagged` carried as cumulative
  `total_lagged`).
- **ADR numbering:** this pair keeps 0032/0033; the net-http construction-surface amendments
  (a separate, unmerged workstream) take **ADR-0034**, not 0032.

## Relationships

Completes the WS resilience stack **ADR-0032** deferred, on the ADR-0029 kernel (`Layer`,
`ErrorKind`, `Timer`) plus the new `Spawn` seam. Mirrors **ADR-0031** (the HTTP sibling) and
inherits its per-attempt-auth and proactive/reactive pacing shape, inverting the circuit breaker
for a must-maintain feed. Feeds the lifecycle channel to **ADR-0004** (risk) and **ADR-0022**
(graduated failure); defers subscription replay and order recovery to the adapter per **ADR-0006**
/ **ADR-0003**; routes `Tracing` to the **ADR-0014** Telemetry plane; rests on **ADR-0007**
(compile-time `impl` seams, no `dyn`). Glossary unchanged — `Spawn`, `ReconnectingConnector`,
`ReconnectingConnection`, `WsControl`, `LifecycleSnapshot` are implementation vocabulary; [CONTEXT.md](../../CONTEXT.md) is
domain-only, and IBKR/Binance/Coinbase values are reference data for the adapters.
