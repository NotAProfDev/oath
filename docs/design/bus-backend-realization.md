# Bus backend realization (reference, not a decision)

How each transport realizes the backend-agnostic Bus **contract** (ADR-0020). The
trait says *what*; this note records *how*. Nothing here is a guarantee the trait
makes — it is the map from one universal contract to per-backend mechanisms, and is
updated as adapters are actually built (none exist yet).

The boundary test for everything below: *does surfacing it buy the consumer a
capability (→ trait, like the loan guard) or is it just how one backend copes
(→ adapter, here)?* All of this is the "copes" side.

## Contract → mechanism

| Contract (ADR-0020) | iceoryx2 | Kafka | Zenoh | Aeron |
|---|---|---|---|---|
| keyed routing | services + **shard-fn + within-shard filter** (service-count limit) | topic **partitions**, key→partition + consumer assignment | **key-expressions + wildcard subs** (native; *no manual shard*) | streams/stream-ids + sub-side filter |
| `LatestValue` keyed store (per-key, depth N) | **blackboard** (per-key slot) | **log-compacted topic / KTable** | **storage + queryable** | per-key **latest-cache** over a stream |
| `Reliable`, never-block, error-on-overflow | reliable service, publish → `Full` | producer non-block mode (`max.block.ms = 0` → error) | reliability QoS + congestion = drop/block → pick error | **`offer()` returns `BACK_PRESSURED`** — natively error-on-overflow |
| zero-copy loan *(capability-gated)* | real shared-mem loan | guard backs owned value | guard backs owned value | guard backs owned value |

## Notes

- **Sharding (ADR-0021 level 1)** is the iceoryx2 / Aeron / Kafka answer to a
  bounded number of cheap channels: `shard = shard_fn(instance-key)` into
  `[0, shard_count)`, both publisher and subscriber computing the same shard (it is
  topic-class config, like QoS); the subscriber filters co-tenant keys on read
  using the **per-key payload seq-stamp**, so gap-detection is shard-transparent.
  `shard_fn = identity` is the 1:1 degenerate case for a small stable universe.
  Default to a simple shard-fn; interest-correlated shards + hot-symbol carve-outs
  are tuning applied when cardinality (`N symbols × M sources × K sub-keys`)
  demands it. **Zenoh skips this layer** (native key-space routing).
- **Aeron sharding runs coarser** than iceoryx2: each publication is its own
  shared-memory log buffer, so the cost pressure pushes toward fewer stream-ids
  (smaller `shard_count`) — same model, different tuning. Aeron IPC is
  1-publisher : N-subscriber (multiple subscriptions per `(channel, stream-id)`),
  which fits OATH's one-Source-per-adapter pattern.
- **`LatestValue` keyed-store depth `N`** is a per-topic knob (default 1 = pure
  latest). `N > 1` ("hashmap-of-rings") is a lossy recent window; "need every one"
  is `Reliable`, not a deeper store.
- **Delta order books** are folded in the **adapter** (ADR-0003 anti-corruption):
  apply venue deltas to a canonical book, publish **snapshots** as `LatestValue`
  (depth 1). Consumers read the current book; the venue's delta protocol never
  leaks inward. A raw-delta `Reliable` stream is an opt-in for microstructure
  consumers.
- **Order-path failure mechanics** (the drain-fast internal buffer, the degraded/
  probe ladder, the two halt modes) are specified in ADR-0022 and realized in the
  broker adapter.
