# Two-level message routing: coarse backend shard + in-process demux

Per-key routing happens at **two levels**. The **backend** delivers at a coarse,
cost-effective granularity (shards on iceoryx2 / Aeron / Kafka, native
key-expressions on Zenoh); a uniform **in-process demux** in the consumer runtime
then fans those messages out to per-key subscribers at memory speed
(`hashmap: key → [subscribers]`). The Bus trait and `subscribe<M>(key)` API are
**unchanged** — consumers never see the demux. This applies to **`Reliable`
streams**; `LatestValue`'s keyed store (ADR-0020) is already read-by-key and needs
no demux.

Cross-process selectivity stays with the backend (do **not** firehose every
process — it is expensive on Aeron / Kafka / networked Zenoh and wakeup-heavy even
on iceoryx2, and it discards the selectivity the network backends need). Fine
fan-out to multiple *same-process* consumers becomes a hashmap lookup instead of K
backend subscriptions. The demux is a **no-op at one-consumer-per-process** (the
MVP one-strategy-per-node default, where it degenerates to the existing sub-side
filter) and **load-bearing for the many-consumers-per-process future** (WASM-density
strategies, ADR-0013's parked sandbox question).

## Considered options

- _Native backend per-key routing everywhere_ — rejected as the default: routing
  config becomes backend-specific and tier-dependent, whereas in-process routing is
  one testable module with identical code on every backend. (A networked
  deployment may still use finer native routing where bandwidth matters — the API
  is unchanged.)
- _Firehose to every process + pure in-process routing_ — rejected: loses
  cross-process selectivity; every process pays for the whole market. Chosen:
  coarse shard (level 1) + in-process demux (level 2). The LMAX/exchange
  "firehose → in-process dispatch" pattern is just this with level 1 set to one
  coarse shard — a tunable special case for a single-host deployment, not the
  general rule.

## Consequences

- **Zero-copy preserved where free, paid where it buys isolation.** Same-thread
  fan-out and K = 1 filtering are zero-copy; cross-thread fan-out to *isolated*
  consumers copies out per consumer — the thread-hand-off boundary of ADR-0020,
  and the price of stopping one slow strategy from stalling its neighbours.
- **The demux is the home of per-key state for same-process consumers**: it holds
  `LatestValue` per-key slots and per-subscriber `Reliable` queues.
- **A truly point-to-point transport** would put fan-out in its adapter
  (relay / MDC), still below the trait.
