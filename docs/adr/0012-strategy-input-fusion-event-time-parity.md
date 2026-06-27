# Strategy input fusion, event-time ordering, and the parity/latency model

The strategy framework delivers a strategy's subscribed Bus topics as a single
**event-time-ordered** merged stream — mandatory for backtest↔live parity — plus a
read-only **latest-value view** (the `StateView` analogue, ADR-0008); windowed /
correlation joins are strategy-owned state for MVP. Live ordering uses a
configurable per-Environment **lateness bound `L`** (default small; `L = 0` for
latency-critical Live), so latency-critical Live can set `L = 0` to avoid
buffering while `L > 0` buffers the live decision path by up to `L`, and
each Environment records its consumed input stream in **ingestion order** (a compact
index over the durable Bus topics) so a recorded run replays bit-exactly regardless
of `L`.

## Considered options

- _Raw ordered merge only_ (strategy hand-rolls all fusion) — too little; every
  strategy re-rolls latest-value boilerplate.
- _Full join / CEP framework_ (windowed, temporal-correlation operators) —
  premature; building Flink inside OATH. Promote specific operators only when a real
  need recurs.
- _Ordered merge + latest-value view_ (chosen): the 80% case, cheaply.
- _Always strict reorder_ (buffer to completeness) — rejected: a stalled adapter
  would freeze the strategy waiting for an event that may never arrive, and it adds
  latency Live cannot pay.

## Consequences

- **Two parity goals, two mechanisms.** Reproducing a _recorded_ run is exact via
  the ingestion-order log — at zero added live latency, because reproduction reads
  the logged order instead of re-deriving it. Predicting from _fresh_ history uses
  event-time + `L` and holds only **above `L`'s granularity**; under-setting `L`
  degrades parity, surfaced as a late-event metric.
- **Late events** (arriving past their watermark) are delivered
  **marked-and-counted, never silently dropped** — dropping market data is
  dangerous, and a deterministic fold is never fed lossy input. A rising late rate
  signals that `L` is set too low.
- The input-stream recording is **async / off-hot-path** and is _not_ ADR-0006's
  log-before-send (which remains a deliberate order-path cost).
- A Strategy is structurally a **Kernel one level out**: a push-driven event-time
  fold over an ordered input stream, with a read-only view, emitting into a sink.
