# Request/reply over the Bus; query tiering and read-isolation

The Frontend's narrow query escape hatch (ADR-0014) and reconciliation queries
(ADR-0006) are served by **request/reply implemented as a thin correlation layer
over the Bus's pub/sub** — a request topic per responder (`core.query.req`,
`supervisor.control.req`, `repo.query.req`) and a reply topic keyed by a
request-id — **not** by a separate side-channel transport. The Bus trait gains no
req/reply method; correlation, reply-to, timeout, and retry live in one
backend-agnostic helper _above_ the trait, so every Bus backend (iceoryx2 local,
RabbitMQ / Kafka networked) carries req/reply for free over a **single backend
matrix**. Queries are **tiered by target**: the push-spine (Business State
snapshot + Domain-Event stream) answers the steady state with no round-trip; live
detail beyond the snapshot is a **non-logged read** the Kernel serves between
inputs (seq-stamped to the input it read at); historical / voluminous data (full
order / fill history) is served by the **repository / Event-Log store, entirely
off Core** (ADR-0009 log↔repository split). A **Frontend read query never enters
the Event Log** — only a reconciliation _response_, which Core folds into state,
does (ADR-0006).

## Considered options

- _A separate side-channel transport for req/reply_ — rejected: a second
  transport trait with its own backend matrix, kept in lockstep with the Bus,
  plus a bespoke network implementation the moment the Bus is networked. It
  doubles transport maintenance merely to keep the Bus trait tidy, and a
  local-socket channel would pin the Frontend to Core's host.
- _A native req/reply method on the Bus trait_ — rejected: request-id correlation
  is supported very unevenly across backends; a trait method bloats the trait and
  constrains backend choice. Built over pub/sub, backends only ever implement
  pub/sub.
- _Logging Frontend read queries as Core inputs_ — rejected: a human's CLI reads
  would alter the deterministic input stream and change Replay. Reads are pure and
  non-logged; the parked req/resp note conflated this with reconciliation, whose
  response genuinely is a folded input.

## Consequences

- **Must-deliver without durable reply topics:** requester timeout + retry over a
  (possibly lossy) reply topic gives effective must-deliver, pairing with
  ADR-0015's responder-side admission-bounding — so a query reply needs no durable
  Bus topic.
- **Uniform backend-agnosticism:** swap to a networked Bus and the same Frontend
  observes a remote host with no extra work; the query interface inherits the
  Bus's reach.
- **The control plane reuses this verbatim** (`supervisor.control.req`):
  operational commands are req/reply too, so they ride the same correlation layer
  — no new mechanism.
- **Logical / physical split:** "query interface" stays a distinct logical API
  (`request → response`) the Frontend programs against, decoupled from pub/sub but
  physically just Bus messages — honoring the glossary's "Bus, and query
  interfaces."
