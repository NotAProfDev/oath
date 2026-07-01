# WebSocket transport contract: untyped duplex frame channel, asymmetric `Stream`/RPITIT split, out-of-band lifecycle

[ADR-0029](0029-network-adapter-stack-transport-split-compile-time-composition.md)
split the net layer by transport, placed `Service` and the request/reply contracts in
`oath-adapter-net-http-api`, and deferred the WebSocket transport to "a deliberate later
session" — guaranteeing only that the kernel was ready for it (`Layer` machinery +
`ErrorKind` + `Timer` all apply unchanged). This ADR is that session: it fixes the
**`oath-adapter-net-ws-api` contract** — the streaming connection-shape the kernel's
`Service` could not model, what it carries, the leaf seam a backend implements, and the
backend — driven by the first [Broker](../../CONTEXT.md)/[Data Provider](../../CONTEXT.md),
IBKR's Client Portal Web API WebSocket. It mirrors [ADR-0030](0030-http-transport-contract-wire-bytes-streaming-composition.md)
for HTTP. The resilience layers that wrap this contract (reconnect, heartbeat, the
bounded-buffer mechanism §6 names) are specified in **ADR-0033**.

## The grounding case — IBKR Client Portal WebSocket

The WS endpoint (`wss://api.ibkr.com/v1/api/ws`, or `localhost:5000` via the gateway)
has **no independent authentication**: it authorizes only by replaying the authenticated
gateway session — the `set-cookie` cookies (gateway mode, e.g. `x-sess-uuid`) or the
`session` value from the REST `/tickle` keepalive. The session idles out after ~5 min
unless `/tickle` is called (~every 60s). So the WS rides **on top of** the REST session,
which is why HTTP (ADR-0030) was built first. Market data is **conflated server-side to
~500ms per instrument** over ~100 subscription lines; `smd` subscriptions **self-terminate
after ~15 minutes** (raised from 10 by IBKR ~2026-04; the server does not auto-unsubscribe on
expiry, so the client `umd+`s then `smd+`s to refresh). Expiry is **silent and per-topic**:
inbound ticks for the affected `conid` simply stop — no close frame, no error — while the
connection, session, and system heartbeat all stay healthy. These are reference data for the
adapter, not domain terms.

## Decision

### 1. Untyped duplex **frame** channel; subscription grammar and demux stay in the adapter

`net-ws-api` is pure frame transport: a connection over which the adapter **sends** frames
(subscription commands) and **receives** a stream of frames. The transport knows nothing
of subscriptions, topics, or `conid`s. IBKR's grammar (`smd+`/`smh+`/`umd-`), JSON parsing,
the `{"topic":"system","hb":…}` application heartbeat, and **demux** of the one multiplexed
frame stream into per-instrument canonical streams all live in `oath-adapter-ibkr`. This is
the [ADR-0030](0030-http-transport-contract-wire-bytes-streaming-composition.md) §1 / ADR-0003
anti-corruption boundary carried into WS: a venue's wire grammar must not leak into a shared
crate, and `net-ws-api` stays reusable for a future non-IBKR streaming feed.

A consequence used throughout: the transport is **grammar-blind**. It cannot distinguish a
market-data frame from an order/execution frame — that classification *is* the demux the
adapter owns — and §5/§6 both turn on this.

### 2. Asymmetric shape: `Stream` recv, RPITIT send, split owned halves

`Service<Req>` models request→one-reply and cannot model "subscribe → many frames over time"
(ADR-0029 §2). The contract is therefore **asymmetric by operation shape** — the same move
ADR-0030 §2 made (buffered request body / streaming response body):

```rust
fn connect(&self, handshake: http::Request<()>)
    -> impl Future<Output = Result<(WsSink, WsSource, Lifecycle), WsError>> + Send;

// recv half — receiving is stream-shaped:
WsSource:  impl Stream<Item = Result<Frame, WsError>> + Send

// send half — sending one frame is request-shaped (one-shot), NOT a Sink:
// `close` takes `self` by value — shutdown is one-way and terminal, so the sink
// cannot be `send`-used after close is requested (enforced by the type system).
trait WsSink { fn send(&mut self, f: Frame) -> impl Future<Output = Result<(), WsError>> + Send;
               fn close(self)               -> impl Future<Output = Result<(), WsError>> + Send; }
```

- **recv is `futures_core::Stream`**, not a hand-rolled pull iterator. This is the
  `http_body::Body` precedent (ADR-0030 §3 took a `poll_frame`/`Pin` streaming trait plus
  `pin-project-lite` and rejected hand-writing the state machine). `impl Stream + Send` is
  monomorphised, zero-box, not object-safe — it honours ADR-0029 §5 (no `dyn`, no
  `async-trait`, no per-call alloc) exactly as `impl Future` does; §5 forbids boxing, not
  poll-shaped traits. It gives the ADR-0033 reconnect/heartbeat layers `StreamExt`/`unfold`
  (async-closure wrapping, idle `timeout`) instead of manual `poll_next`.
- **send is RPITIT one-shot**, mirroring `HttpClient::send` — deliberately **not** `Sink`,
  whose `poll_ready`/`start_send`/`poll_flush` *is* the poll-handshake the `Service` design
  walked away from. Subscribe/heartbeat traffic is low-volume; no `Sink` backpressure
  handshake is warranted.
- **Split owned halves.** `connect` yields a send half and a recv half that move to separate
  tasks (IBKR needs concurrent send of subscribe/heartbeat and receive of frames). They are
  single-owner — recv is exclusive `&mut self` (inherent in `Stream::poll_next`'s
  `Pin<&mut Self>`), not the shared `&self` of `Service`. This is the identity-wrap of
  tungstenite's `WebSocketStream::split()` (the ADR-0030 §7 "leaf is nearly an identity
  wrap" criterion, applied to WS).

### 3. `Frame` is a minimal enum; the default stack hands the adapter only data frames

```rust
enum Frame { Text(Bytes), Binary(Bytes), Ping(Bytes), Pong(Bytes), Close(Option<CloseFrame>) }
```

"Untyped" (§1) means *no venue/JSON typing* — not flattening WebSocket's own protocol frame
kinds, which are transport concerns (RFC 6455), not venue concerns. The enum is the
**leaf/inter-layer** vocabulary. After the ADR-0033 default stack, the **adapter-facing
`WsSource` delivers only `Text`/`Binary` data frames**: the heartbeat layer absorbs protocol
`Ping`/`Pong` (auto-Pong) and `Close` becomes a lifecycle transition (§4). IBKR's
*application* heartbeat is a `Text` frame, so it reaches the adapter — that is venue liveness,
handled adapter-side. Crucially, **control frames bypass the §6 data buffer** and are answered
regardless of consumer drain speed, so a slow data-consumer never starves the Pong that keeps
IBKR from dropping us.

### 4. Lifecycle is a separate, epoch-stamped channel — not a widened frame item

Connection health is a **third handle**, not an item interleaved into the data stream:

```rust
Lifecycle: a last-value channel of ConnState   // watch-style; delivery form resolved in ADR-0033
enum ConnState {
    Connected { epoch: u64 }, Stale, Reconnecting, Resumed { epoch: u64 },
    Unrecoverable,                // a classified non-transient failure — will not self-heal (ADR-0033 §7)
}
// Buffer overflow (§6) is NOT a connection phase — it is orthogonal to `ConnState`
// (a connection can be `Connected` *and* lagging). The `Lagged` signal is carried as
// the monotonic cumulative `total_lagged` field on the ADR-0033 §5 `LifecycleSnapshot`,
// not as a variant here — see the delivery-form note below and §6.
```

ADR-0033 resolves the delivery form (a `watch` of an epoch-stamped `LifecycleSnapshot`, not a
transition stream — its §5 explains why, and why `Lagged`'s count is carried as a
monotonic cumulative total under last-value semantics). `Unrecoverable` is emitted by the
resilience layer when it classifies a permanent failure rather than retrying it forever
(ADR-0033 §7); it is the one terminal state — every other variant is transient.

- **The data stream stays `Result<Frame, WsError>` (§2), uncontaminated by control variants.**
- **The feed-*down* edge is first-class.** For a trading system the safety-critical event is
  the feed going *stale*, not its recovery: a stale order/exec stream means we may be blind on
  fills and must stop issuing and let risk react. `Stale`/`Reconnecting` are therefore signals
  in their own right, feeding [ADR-0004](0004-risk-as-continuous-control-loop.md) (risk control
  loop) and [ADR-0022](0022-reliable-order-path-graduated-failure.md) (graduated failure) — not
  just `Resumed`.
- **It is the shared signal plane for the ADR-0033 layers** — the reconnect layer emits
  `Resumed`, the heartbeat layer emits `Stale`, the buffer layer emits `Lagged` (§6). This is
  the WS analogue of what `ErrorKind`/Telemetry is to the HTTP stack.
- **Epoch-stamped.** `Resumed{epoch}` bounds the adapter's reconcile window ("reconcile since
  epoch N") and disambiguates an in-flight `umd-` queued against the dead connection vs. the
  new one. Ordering is free: a fresh session is silent until resubscribe, so `Resumed` strictly
  precedes any post-reconnect frame and `Stale` strictly follows the last pre-drop frame — the
  correlation an in-band design would buy is guaranteed by the protocol regardless of channel.

### 5. Recovery is split: transport re-establishes the connection, adapter replays subscriptions

Because the transport is grammar-blind (§1), it **cannot** replay subscriptions — it does not
understand `smd+`, and a blind replay-log of sent frames would resurrect `umd-`'d
subscriptions. So:

- The **transport** (ADR-0033 reconnect layer) rebuilds TCP + the WS handshake, **re-injects
  auth** (§8), bumps the epoch, and emits `Resumed{epoch}`.
- The **adapter** owns subscription replay and the **differential** recovery only it can make,
  because only it knows which stream a frame belongs to:
  - **Market-data** streams resubscribe and accept the gap — a resubscribe returns the current
    book (a fresh `LatestValue`, [ADR-0020](0020-bus-trait-delivery-classes-access-patterns.md)),
    so the gap self-heals (cf. [ADR-0002](0002-backend-agnostic-bus-canonical-message-model.md):
    "acting on stale-but-delivered messages is a separate, consumer-side freshness concern").
  - **Order/exec** streams cannot merely resume — fills may have occurred during the gap — so
    the adapter runs a REST **reconciliation** pass
    ([ADR-0006](0006-broker-reconciliation-contract.md)).

  The same `Resumed` (or `Lagged`, §6) signal thus drives two different adapter responses. This
  is where the WS and REST transports meet in `oath-adapter-ibkr`, exactly as ADR-0029 foresaw.
  IBKR's ~15-minute `smd` self-termination gives resubscription a **second trigger** beyond
  reconnect: a periodic refresh timer the adapter owns.

### 6. Backpressure: a uniform, no-silent-drop guarantee at the transport; per-stream policy in the adapter

"What happens when a `WsSource` is not drained" is a property the adapter codes against, so it
is **contract**, not a resilience detail. A WS subscription is push-based — IBKR sets the rate,
we set the consume rate — so frames can queue. Two facts fix the guarantee:

- **Grammar-blindness (§1)** means the transport *cannot* apply a per-stream policy — it can't
  tell an MD frame from an order frame. So the transport guarantee is necessarily **uniform**.
- A grammar-blind transport that silently dropped could discard an **execution report** — the
  WS analogue of the duplicate-order incident `Retry` was designed around. So **silent drop is
  out**.

The guarantee: **the transport never silently discards; on overflow it drops oldest data frames
and signals the drop by advancing the cumulative `total_lagged` (the `Lagged` signal, §4).** The **per-stream drop/keep policy lives adapter-side, after
demux** (MD → `LatestValue` drop-to-latest, ADR-0020; orders → reliable handling +
reconcile-on-`Lagged`). Control frames bypass the buffer (§3). Two distinct "latest" mechanisms
at two layers — coarse, grammar-blind drop-oldest at the transport (hence the signal); semantic
per-`conid` overwrite downstream.

Rationale to record: with IBKR specifically, overflow from *broker* volume is unlikely
(conflation @500ms × ~100 lines ≈ ~200 small msgs/sec, trivially drained). `Lagged` exists for
**consumer-side stall correctness** — if *our* demux stalls (a blocked downstream, a scheduler
hiccup), we must not silently lose an order frame — not as throughput defence. (TCP-backpressure
— refusing to read the socket — is rejected: it stalls *all* subscriptions and the Pong, and is
wrong for a market feed that wants freshest-wins.) The buffer *mechanism* is ADR-0033; the
*guarantee* is here.

### 7. Leaf seam and backend: `WsConnector` over tokio-tungstenite + rustls

The named dependency-inversion seam the adapter codes against — the `HttpClient` analogue — is
the `connect` of §2, exposed as a `WsConnector` trait. The WS upgrade *is* an HTTP GET, so the
handshake is an `http::Request` (reusing the `http` crate, consistent with `net-http-api`). The
first leaf backend is **`oath-adapter-net-ws-tungstenite`** (tokio-tungstenite over rustls) — the
analogue of `net-http-hyper` — whose `WebSocketStream::split()` is the near-identity source of
`WsSink`/`WsSource`. Per ADR-0029 §5 it is a **compile-time `impl WsConnector` seam**, not `dyn`.

### 8. Auth: a per-transport `AuthSource` trait, one shared impl, re-pulled per (re)connect

`AuthSource` is the seam that lets venue- and scheme-neutral net layers apply *current*
credentials to an outgoing request/handshake without learning the scheme — gateway mode stamps a
`Cookie`; OAuth 1.0a stamps a signed `Authorization` header; swapping them is swapping the
`impl`. It operates on `http::request::Parts` (method + uri + headers — body-agnostic, so the
same shape serves HTTP's `Request<Bytes>` and the WS `Request<()>`).

`AuthSource` is **not** hoisted to the kernel: it touches `http` (forbidden in the std-only
kernel, ADR-0029 §3), and it is shared by HTTP + WS but **not universal** (a future FIX/multicast
transport authenticates differently) — the same category as `Service`, which ADR-0029 kept out
of the kernel. So the small trait is **declared per-transport** (in `net-http-api` and
`net-ws-api`), and IBKR's single `IbkrAuthSource` (one gateway session, one `/tickle` loop)
implements both — no extra crate. The WS reconnect layer calls it **per (re)connect** (never
caching at first connect), so a session refreshed by `/tickle` between drop and reconnect is
picked up — the streaming analogue of ADR-0031 §1's per-attempt re-stamp.

## Considered options

- *Reuse `Service` for WS* — rejected by ADR-0029 §2; request→one-reply cannot model a
  subscription's many frames.
- *Typed per-subscription stream API (transport owns demux)* — rejected: demux-by-topic is venue
  grammar; owning it breaches the §1/ADR-0003 boundary and couples `net-ws-api` to IBKR.
- *Hand-rolled RPITIT pull `recv()` for uniformity with `Service`* — rejected: it reads as "stay
  free of streaming-contract deps," but `net-http-api` is *not* free of them (it took
  `http-body` + `pin-project-lite`); the blessed pattern is the ecosystem streaming trait, and
  `Stream` honours ADR-0029 §5 just as `Body` does. (A `recv()` would also make the leaf
  hand-crank an iterator over tungstenite's `Stream`, forfeiting the identity wrap.)
- *`Sink` for the send half* — rejected: its `poll_ready`/`start_send`/`poll_flush` is the
  poll-handshake the contract avoids; one-shot `send` suffices for low-volume control traffic.
- *Opaque `Bytes` frames* — rejected: erases the `Text`/`Binary` distinction and forces
  protocol `Ping`/`Pong`/`Close` up into the adapter.
- *Widen the recv item to `Frame | Resumed | …` for lifecycle* — rejected: smears the control
  plane into the data plane and re-widens the §2 stream; and a `Resumed`-only widening would
  emit the "all clear" while staying silent on the safety-critical feed-down edge.
- *Transport replays subscriptions on reconnect* — rejected: requires grammar (which `umd-`
  cancels which `smd+`), the §1 leak; replay is the adapter's because only it holds the
  subscription set and the order-vs-MD recovery distinction.
- *Silent drop-to-latest at the transport, or terminal `WsError` on overflow* — rejected:
  silent drop can lose an execution report (grammar-blind); a terminal error tears down *every*
  subscription on a transient consumer hiccup (reconnect storm). Drop-oldest + `Lagged` keeps the
  connection and signals the one thing the adapter needs.
- *`AuthSource` in the kernel, or in a shared `net-auth-api` crate* — rejected: kernel is
  std-only and `AuthSource` needs `http` and is non-universal; a dedicated crate is unwanted
  ceremony for a one-method trait. Per-transport declaration with one shared impl gives "one
  session feeds both" without either.

## Consequences

- **New crates:** `oath-adapter-net-ws-api` (contract; deps `http`, `bytes`, `futures-core`,
  `futures-util`, `pin-project-lite`, `thiserror`, `tracing` — the mirror of `net-http-api`'s
  set; still zero-I/O, zero-runtime) and the leaf `oath-adapter-net-ws-tungstenite` (the only
  `tokio`/`tokio-tungstenite`/`rustls` dependency, owning the `tungstenite::Error → WsError` and
  `Message → Frame` mappings). `WsError` implements `HasErrorKind` once (close codes / connection
  failures → `ErrorKind`).
- **`AuthSource` is declared in both `net-http-api` and `net-ws-api`** (identical one-method
  trait); the README graph already lists `oath-adapter-net-api` as "HTTP/WS composition
  primitives" and is updated to show the per-transport crates when they are built (deferred, as
  for ADR-0029–0031).
- **The adapter (`oath-adapter-ibkr`) owns:** the subscription grammar, JSON/frame parsing,
  demux, subscription replay + the periodic ~15-min `smd` refresh, the differential
  `Resumed`/`Lagged` recovery (MD resubscribe vs. order REST-reconcile, ADR-0006), the per-stream
  backpressure policy, and the single `IbkrAuthSource`.
- **The lifecycle channel becomes a first-class input to risk/order control** (ADR-0004 /
  ADR-0022), not merely an internal reconnect detail.
- **Recv-side backpressure is settled here** (§6): the *guarantee* (drop-oldest data + `Lagged`,
  control bypasses) is unchanged by ADR-0033, which only refines how the count is carried under
  the §4 last-value channel (cumulative `total_lagged`) and adds the dual count+byte bound.

## Relationships

Fills the WebSocket contract **ADR-0029** deferred, on its kernel (`Layer`/`ServiceBuilder`,
`ErrorKind`, `Timer` unchanged). Mirrors **ADR-0030** (the HTTP sibling) and reuses its
`AuthSource` seam (**ADR-0031** §1 per-attempt re-stamp). Rests on **ADR-0007** (in-process ⇒
compile-time `impl` seam, no `dyn`) and **ADR-0003** (anti-corruption: grammar/typing in the
adapter). Recovery defers to **ADR-0006** (reconciliation) and reads delivery semantics from
**ADR-0020** / **ADR-0002**; the lifecycle channel feeds **ADR-0004** / **ADR-0022**. Is the
base for **ADR-0033** (the WS resilience stack: reconnect, heartbeat, the §6 buffer layer, and
the default layer order). Glossary unchanged — `Frame`, `WsSource`, `Lifecycle`, `WsConnector`,
`AuthSource` are implementation vocabulary, and [CONTEXT.md](../../CONTEXT.md) is domain-only;
IBKR WS values are reference data for the adapter.
