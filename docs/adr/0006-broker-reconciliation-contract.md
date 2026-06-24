# Crash recovery: broker-authoritative reconciliation

After a crash, Core recovers in two steps: **replay** the Event Log to rebuild
decision state (side effects suppressed), then **reconcile** against the broker,
which is the **single source of truth** for what actually happened to orders,
positions, and fills. Replay restores intent; reconciliation restores reality;
Core repairs any divergence into its recovered state.

## The reconciliation join key

Every order carries a **client order id** — the join key that lets Core ask the
broker a precise question ("what happened to `abc123`?") instead of matching by
ambiguous attributes (symbol/side/qty/price). Because the kernel is
deterministic, this id is regenerated identically on replay, so an order decided
before a crash is matched exactly to the broker's report. Without the id,
reconciliation degrades to fuzzy attribute matching and silently repairs
state incorrectly whenever two similar orders or partial fills exist.

## The ordering invariant (write-ahead)

The input(s) that trigger an order decision **must be durably appended to the
Event Log before the resulting order is transmitted to the broker.** Otherwise
replay cannot regenerate the order (or its client order id) and the broker is
left holding an unmatchable orphan order. A *separate* write-ahead "decision
record" is not needed — replaying the inputs regenerates the decision — but this
log-before-send ordering is mandatory.

## Mandatory adapter capability

Every **Broker** adapter MUST provide (a) **idempotent submit** keyed by a
client order id (deduping retransmissions), and (b) a **queryable** view of
order / position / fill state for reconciliation. Venues lacking native support
must emulate it inside the adapter (persisted id mapping, dedup table). Venues
where this cannot be made to hold are excluded — a deliberate scope boundary
favouring a clean, trustworthy core over maximal venue coverage.
