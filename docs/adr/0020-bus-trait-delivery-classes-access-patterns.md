# Bus trait: two delivery classes × two access patterns over one canonical model

The Bus (ADR-0002) is one trait carrying a family of typed canonical messages, and
it offers exactly **two delivery classes**, each with its own **access pattern**:

- **`LatestValue` — a keyed store.** Lossy / overwrite-allowed, **per-instance-key
  isolated**: each key owns a slot of depth `N ≥ 1` (default 1; `N > 1` is a lossy
  recent window). Accessed **read-by-key (+ change-notification)**, never as a
  filtered firehose — so a high-volume key can never evict ("starve") a low-volume
  key's latest, and a slow reader misses intermediate values but always reads the
  current one. Publishing never blocks. Models current price/quote, order-book
  snapshot, position/P&L.
- **`Reliable` — an ordered stream.** No message is silently dropped; on a full
  bounded queue `publish` returns an explicit error (never blocks). Accessed
  **subscribe/receive**. Models every tick/fill/order/Domain Event.

**Bounds & ownership.** The **universal type bound is `Serialize`** plus `Message`
metadata (its `Key`, role, delivery class). **Zero-copyability is a backend
capability, not a trait requirement** — iceoryx2 concretizes it as `#[repr(C)]` POD
(`ZeroCopySend`); other zero-copy transports impose their own layout discipline.
OATH's *own* canonical messages additionally satisfy POD + `Serialize` by
convention so they ride every backend including the fast path; user-defined
messages need only `Serialize` (and opt into a backend's discipline for its fast
path). **Receive is a loan guard** (`Sample<M>: Deref<Target = M>`, drop returns
the slot) — a real shared-memory loan where the backend supports it, an owned value
otherwise. A loaned `Sample<M>` **may not cross** retention, thread hand-off, or
`.await`; the consumer **materializes it into an owned payload** before each
boundary, then drops the loan (owned payloads move normally — `Copy` POD is a
backend-specific fast-path discipline, not a public-API bound). (Core's drain — read field, append bytes to the Event Log, fold,
drop — is the inspect-and-discard hot path that earns the guard its public place.)
**Send is `publish(&M)`**; **loan-to-write (`SampleMut<M>`) is an opt-in**
zero-copy-construct path for large/bulk payloads (order-book depth, history pages).

**Identity & routing.** A topic is `(Environment namespace, payload type, role,
instance-key)`. `role` is the QoS-bearing channel (e.g. `bars.live` vs
`bars.history`); the **shard is *not* an identity coordinate** — it is derived from
the instance-key by the backend. The **instance-key is a typed, per-type `Key`
struct** (`QuoteKey { source, symbol }`, `BarKey { source, symbol, interval }`,
`FillKey { source }`, `OrderKey { account }`, …) with a canonical, escape-safe wire
encoding produced once inside the mapping layer — never hand-concatenated at call
sites. **Key granularity = consumer-interest granularity**: `LatestValue` stores
are read by fine `Symbol`-level keys; whole-consumed `Reliable` streams (fills,
orders, events) collapse to coarse `(Source/role)` keys; a per-`Symbol` `Reliable`
stream keeps the `Symbol`. A **per-key sequence stamp lives in the payload**, so
gap-detection is identical whether a key has its own channel or shares one.

**Excluded — bootstrapping is not a Bus concern.** Acquiring state established
before a participant was listening (late-join) is the same question as crash
recovery, answered by the recovery/query substrate (ADR-0005 / 0006 / 0016) — never
by a "history" delivery knob.

## Considered options

- _One canonical envelope enum on shared topics_ — rejected: forces max-variant
  slot size across a C ABI and prevents per-topic delivery semantics. Chosen: a
  family of typed POD topics + an open, user-extensible `Message` trait (a category
  enum is permitted as just another `M`).
- _`Pod + Serialize` as the universal bound_ (ADR-0002's phrasing) — refined:
  `Serialize` is universal; POD is one backend's zero-copy discipline.
- _One delivery class delivered as a filtered stream_ — rejected: a shared lossy
  ring lets a fast key starve a slow one, so "subscribe to AAPL, never see AAPL"
  becomes possible. Chosen: `LatestValue` is a per-key-isolated keyed store, read
  by key.
- _Owned copy-out as the public receive API_ — rejected: forecloses zero-copy on
  Core's hot path. Chosen: the loan guard (degrading to owned) + mandated copy-out
  at the three boundaries.

## Consequences

- **Refines ADR-0002**: `Serialize` is the universal bound (POD backend-specific);
  per-topic delivery becomes two classes × two access patterns; the "stricter
  ownership contract" is the loan guard, the layout discipline is not.
- **Delta order books fold in the *adapter*** (ADR-0003 anti-corruption): the
  adapter applies venue deltas to a canonical book and publishes **snapshots** as
  `LatestValue`; consumers read the current book and never see the venue's delta
  protocol. A raw-delta `Reliable` stream is an opt-in.
- **No backend leaks into the contract** except the deliberate loan-guard exposure:
  sharding, the keyed-store implementation (blackboard / compacted topic / storage
  / cache), ring sizing, and delta math live below the trait — see
  [the backend-realization note](../design/bus-backend-realization.md).
- **`Reliable` routing is ADR-0021; `Reliable` order-path failure is ADR-0022.**
