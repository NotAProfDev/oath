# Backend-agnostic Bus with one canonical message model

Processes communicate over a **Bus** abstracted behind a single trait, so the
transport backend is swappable (zero-copy shared memory such as iceoryx2 for the
fast path; Unix sockets or Kafka for other users). There is exactly **one
canonical message model** shared by every backend — backends differ only in how
they move bytes, never in the data they carry.

> _Refined by **ADR-0020**: the universal type bound is `Serialize` (POD is a
> backend-specific zero-copy discipline, not a trait requirement), and the
> per-topic delivery semantics below are realized as two classes (`LatestValue` /
> `Reliable`) × two access patterns (keyed store / stream)._

## Considered options

- *Per-backend data models* — rejected: it recreates, one layer up, the
  representation swamp that ADR-0003 outlaws in the core.
- *One model, intersection of backend constraints* — chosen.

## Consequences

- Message payloads are designed to the **intersection** of backend constraints:
  fixed-layout, `#[repr(C)]` plain-old-data (for zero-copy) **and** serializable
  (for network backends) — roughly a `Pod + Serialize` bound.
- The trait must be designed to the **stricter** ownership contract: zero-copy
  hands back a *loaned*, lifetime-bounded sample; network backends hand back an
  *owned* value. Modelling the borrowed/lifecycle-bounded case keeps zero-copy.
- `#[repr(C)]` payloads are brittle under schema evolution; versioning is a
  known open concern to be addressed separately.
- Delivery semantics are per-topic: market data is drop-to-latest (ring buffer);
  orders and fills are reliably delivered (never dropped on the wire). Acting on
  stale-but-delivered messages is a separate, consumer-side freshness concern.
- **Durability is a backend capability, not a Bus guarantee.** iceoryx2 is
  ephemeral (zero-copy, fast, lost on crash); Kafka and Chronicle Queue are
  durable replayable logs. A durable backend can double as Core's event log /
  recovery journal (see ADR-0005).
- Pub/sub backends support **dynamic publishers/subscribers**, so adapters and
  strategy nodes can be added or removed at runtime without restarting Core
  (see ADR-0001).
