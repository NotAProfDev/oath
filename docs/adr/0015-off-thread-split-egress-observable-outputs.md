# Off-thread, split egress for Core's observable outputs

Core's observable outputs — ADR-0014's **Business State** and **Domain Events**,
plus **query responses** — leave the Kernel with **no serialization or publish on
the single-writer thread**: the Kernel writes into in-process egress structures
and a small **non-blocking forwarder** thread (compile-time static, in the Core
process per ADR-0007) drains, serializes, and publishes to the Bus. Egress is
**split by delivery need**, which makes memory-boundedness _structural_ rather
than hoped-for:

- **Business State → one coalescing latest-value slot.** A single atomic
  `StateView` snapshot the Kernel overwrites (O(1), never blocks). One slot, not
  per-field — so positions / P&L / exposure are never a torn cross-section of what
  is canonically one fold. **Seq-stamped** with the input it reflects, so a frozen
  sequence reads as a _stalled producer_, not a steady value.
- **Domain Events → a must-deliver, ordered, durable channel.** Never coalesced —
  a heartbeat must never clobber a breach.
- **Query responses → a must-deliver, admission-bounded channel** keyed by
  request-id. Depth-tiny (≈1 query outstanding at CLI rates); overflow **refuses
  admission** of a new query, never drops a response.

The enqueue is **unconditional** and the writer-thread code path is
**byte-identical live vs Replay** — under Replay the forwarder is wired to a null
sink — so the fold stays pure and no `if replay` branch sits on the hot path.

## Considered options

- _Kernel serializes + publishes inline_ — rejected: serialization and Bus I/O on
  the single-writer thread is exactly the hot-path contention ADR-0005 offloads.
  Egress is just another offloaded I/O.
- _One unified egress queue, one policy_ — rejected: any single policy is wrong
  for one payload. Coalescing drops Domain Events; a must-deliver FIFO lets a
  state/telemetry burst grow unbounded or evict a pending query answer. The split
  gives each payload its correct, independently-bounded policy.
- _Per-field Business State slots_ (separate slots for positions, P&L, exposure) —
  rejected: independent slots can publish a torn cross-section (new position,
  stale P&L). One atomic snapshot is simpler and consistent.
- _Drop query responses on overflow_ — rejected: a human asked; the answer cannot
  be silently coalesced away. Bound _admission_ instead, so "must-deliver" stays
  honest.

## Consequences

- The writer thread's only egress cost is a cheap **enqueue / overwrite of an
  owned compact value**; serialization happens on the forwarder.
- **Business State is lossy-by-design** (latest wins) and that is _correct_ —
  observers want latest; the seq stamp distinguishes "steady" from "stalled."
- **Telemetry does not use this path** (ADR-0014): it is out-of-fold and
  self-reported per process, with its own publish path. This egress is for
  fold-products only.
- The coalescing slot and the durable Domain-Event channel are the natural inputs
  for a future read-model projector (ADR-0014) — state side and event side
  respectively — with no new Core emission.
