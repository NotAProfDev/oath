# Single-host, multi-process topology with fault isolation

OATH runs as several cooperating processes on a single host: one process per
**Adapter** (broker or data provider), one **Core** process (risk engine, order
execution, portfolio, and initially strategies), and one or more **Strategy
Node** processes. We chose this over a single-process monolith so that a flaky
external integration — the thing that actually hangs, leaks, or gets
rate-limited — cannot crash the decision-making centre, and over a
multi-machine distributed system because a retail, single-box deployment lets us
use shared-memory IPC for near-in-process latency.

## Consequences

- Fault isolation: an adapter crash takes down only that venue's connectivity.
- The **zero-copy fast path** (shared-memory IPC, e.g. iceoryx2) requires all
  processes on one host. Network Bus backends (Kafka, RabbitMQ, …) relax this:
  processes may live on different machines at a latency cost. Cross-machine
  distribution is therefore supported via those backends, just not on the
  optimized path. (See ADR-0002.)
- The dominant design concern becomes the inter-process contract (see ADR-0002),
  not in-process trait dispatch.
- Strategies start co-located in Core and split into Strategy Nodes later; the
  strategy↔Core seam is designed as a swappable call from day one.
- Adapters **and** Strategy Nodes are hot-pluggable: a new venue or strategy
  process can join the Bus at runtime (subject to symbology resolution and a
  registration handshake) without restarting Core — something a single-process
  design cannot do.
