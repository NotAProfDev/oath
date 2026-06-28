# Network adapter stack: transport-split crates and compile-time `Service`/`Layer` composition

[ADR-0009](0009-crate-topology-spine-inverted-process-aligned.md) placed
`oath-adapter-net-api` in the topology as "HTTP/WS composition primitives," and the
skeleton shipped a Tower-shaped composition core (`Service`, `Layer`,
`ServiceBuilder`, `Stack`, `Identity`) plus a coarse `ErrorKind` / `HasErrorKind`
classifier in one crate. Landing the first [Broker](../../CONTEXT.md) — Interactive
Brokers' Client Portal Web API — forces the question that skeleton deferred: *one net
crate, or many?* This ADR **splits the net layer by transport**, fixes what is
universal enough to live in a shared kernel versus what is transport-specific, and
states the binding-time discipline the whole stack inherits. The HTTP data plane and
the resilience/pacing layers are specified in
[ADR-0030](0030-http-transport-contract-wire-bytes-streaming-composition.md) and
[ADR-0031](0031-http-resilience-venue-pacing.md); this ADR is the structural,
**cross-transport** decision they both rest on.

## Decision

### 1. Split by transport, over a transport-neutral kernel

A venue Adapter speaks more than one wire protocol — IBKR's Client Portal is REST
**and** a streaming WebSocket — and those protocols have *fundamentally different
interaction shapes* (§2). So the net layer is not one crate; it is a kernel plus one
contract crate per transport, plus leaf backends:

```text
oath-adapter-net-api            kernel — transport-neutral, std-only:
                                  Layer, ServiceBuilder, Stack, Identity,
                                  ErrorKind, HasErrorKind, Timer
   ├── oath-adapter-net-http-api   HTTP/REST contracts  (depends on net-api)
   │     └── oath-adapter-net-http-hyper   leaf backend (hyper-util + rustls)
   └── oath-adapter-net-ws-api     WebSocket contracts  (future; depends on net-api)
         └── oath-adapter-net-ws-<backend>   leaf backend (future)
```

This preserves ADR-0009's spine-inverted direction: `net-api` is the most-depended-on
contract, the per-transport `*-api` crates are narrower contracts on top, and concrete
backends implement them. A future gRPC/FIX/multicast transport is a new `*-api` crate
on the same kernel, never a fork.

### 2. `Service` is not universal — it lives in `net-http-api`, not the kernel

`Service<Req>` models **request → one reply**. That fits REST and unary RPC, but it
does **not** model the other transports' core operation:

| Kernel symbol | REST | WebSocket | FIX / TCP session | UDP multicast (recv-only) |
|---|---|---|---|---|
| `Layer` / `ServiceBuilder` / `Stack` / `Identity` | ✓ | ✓ | ✓ | ✓ |
| `ErrorKind` / `HasErrorKind` | ✓ | ✓ | ✓ | ~ |
| `Service<Req>` (request→one reply) | ✓ | ✗ subscription yields *many* frames | ✗ async session | ✗ no request at all |

A WebSocket subscription is "subscribe → stream of frames"; a multicast feed is pure
receive. Forcing those into request/reply is a lie. So **`Service` is a
*connection-shape* contract, not a kernel primitive**, and it lives in `net-http-api`
(the first request/reply transport). WS will define its own streaming contract in
`net-ws-api`. The explicit **no**: there is no shared `Service` in the kernel.

`Service` is transport-*neutral* (it names no HTTP type), so were a second request/reply
transport to appear (gRPC, SSE), `Service` is hoisted into a `net-req-reply-api` crate
shared by both — *not* left in `net-http-api` forcing gRPC to depend on HTTP. We do not
build that crate now (YAGNI); we have one request/reply transport.

### 3. The composition machinery is `Service`-free and stays in the kernel

`Layer<S>` carries **no `Service` bound** — it wraps *anything*. `ServiceBuilder`,
`Stack`, and `Identity` likewise compose an arbitrary `S`. That is exactly why they
belong in the kernel: the same machinery composes an HTTP `Service` stack today and a
WS subscription stack tomorrow. `ErrorKind` / `HasErrorKind` are coarse, wire-neutral
classifications (`Timeout`, `Connection`, `Throttled`, `Auth`, `Client`, `Server`,
`Unknown`) every transport's layers branch on; they stay in the kernel too.

With `Service` removed, the kernel carries **no external dependencies** — std only. A
dependency-free kernel is the signal that the cut is clean.

### 4. `Timer` is a kernel contract, not a runtime

Timing layers (`Timeout`, `Retry` backoff, `RateLimit` refill, `CircuitBreaker`
cooldown — ADR-0031) need a clock, which collides with the `*-api` crates' zero-runtime
charter. Resolution: a minimal **`Timer` trait in the kernel**

```rust
pub trait Timer: Clone + Send + Sync {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send;
    fn now(&self) -> Instant;          // token-bucket / cooldown elapsed-time reads
}
```

A trait is not a runtime, so the charter holds and the kernel stays std-only. Timing
*logic* lives with the transport that uses it (`Timeout`/`Retry`/`RateLimit` wrap a
`Service`, so they live in `net-http-api`), generic over `net-api::Timer`; the
**tokio-backed `Timer` impl lives in the leaf backend** (`net-http-hyper`). `Timer` is
in the kernel rather than `net-http-api` because WS reconnect/heartbeat will need the
same clock — it names no transport. Bonus: a mock `Timer` makes every timing layer
deterministically testable without real sleeps.

### 5. Compile-time binding, no `dyn` — RPITIT throughout

`Service::call` returns `impl Future` (RPITIT) — no `async-trait`, no `dyn`, no
per-call allocation. That makes `Service` (and the `HttpClient` seam built on it,
ADR-0030) **not object-safe**, which is correct here: the network backend is
**in-process**, and [ADR-0007](0007-binding-time-runtime-pluggable-iff-cross-process.md)
binds in-process collaborators at **compile time**, reserving runtime/`dyn` pluggability
for cross-process seams (the Bus). Adapters bind `impl HttpClient` statically. No
boxing, monomorphised stacks.

## Considered options

- *One net crate for all transports* — rejected: it would either force HTTP and WS to
  share a `Service` that WS cannot honour, or accrete two unrelated core traits in one
  crate. The transport split makes the request/reply-vs-streaming fault line a crate
  boundary instead of a comment.
- *Keep `Service` in the kernel as "the" primitive* (the Tower bet) — rejected: it
  reads as universal when it is request/reply-only, and a kernel that advertises
  `Service` invites WS code to treat subscriptions as request/reply, which they are not.
- *Push `Service` into a `net-req-reply-api` crate now* — rejected as premature: correct
  the day a second request/reply transport lands, but today it is one extra crate for a
  generalisation we do not have. `Service` sits in `net-http-api` until then.
- *Timing layers in the backend on `tokio::time` directly* — rejected: it scatters the
  layer logic across crates and couples it to tokio, forfeiting the mock-clock testability
  and the WS reuse the `Timer` trait buys for one trait definition.
- *`dyn`-dispatched layers / `BoxFuture`* — rejected: an in-process seam under ADR-0007
  has no need for runtime pluggability, and `async-trait`'s per-call box is exactly the
  allocation RPITIT removes.

## Consequences

- **The skeleton `oath-adapter-net-api` is repartitioned**: `Service` and the
  `http`/`http-body`/`bytes` deps move out to `net-http-api`; the kernel loses all
  external deps and gains `Timer`. New crates `oath-adapter-net-http-api`,
  `oath-adapter-net-http-hyper`, and (later) `oath-adapter-net-ws-api` join the
  workspace; the README dependency graph is updated to match.
- **The kernel is the single home for cross-transport vocabulary** — composition,
  classification, and time. Each transport crate adds only its own connection-shape
  trait and the layers/types that name its wire format.
- **`oath-adapter-api` is unaffected and independent.** The `Broker` / `DataProvider`
  role traits + host harness speak `oath-model` and do **not** depend on the net crates;
  the concrete `oath-adapter-ibkr` is the only place the inward role contract and the
  outward net plumbing meet (ADR-0003 anti-corruption boundary).
- **WS is a deliberate later session** — its streaming contract, reconnect/heartbeat
  layers, and backend are out of scope here; this ADR only guarantees the kernel is
  ready for it (`Layer` machinery + `ErrorKind` + `Timer` all apply unchanged).

## Relationships

Refines **ADR-0009** (gives `oath-adapter-net-api` its internal structure and adds the
per-transport `*-api` crates). Rests on **ADR-0007** (in-process ⇒ compile-time/`impl`
binding, no `dyn`) and **ADR-0003** (adapter anti-corruption: the net layer is outward
plumbing, role translation stays in the concrete adapter). Is the base for
**ADR-0030** (HTTP transport contract) and **ADR-0031** (HTTP resilience & pacing).
Glossary: no change — `Service`, `Layer`, `HttpClient`, `Timer` are implementation
vocabulary, and [CONTEXT.md](../../CONTEXT.md) is domain-only.
