# IBKR CPAPI write-path wire layer (order place / cancel / status) (2026-07-11)

The **second slice of the IBKR venue adapter**: extend the existing `cpapi` module
with the **order write path** as a transport-agnostic, IBKR-internal **wire layer** —
request-body serde DTOs, response DTOs (including IBKR's order/reply confirmation
union), and `Endpoint` descriptors that mirror Client Portal API v1 (CPAPI) JSON
verbatim. This slice **captures real paper-gateway responses in-band**: it drives the
stateful order dance (place → confirm → cancel) against a logged-in paper account and
reconciles the DTOs to that live wire.

- **Status:** design — awaiting review, then implementation plan.
- **Scope:** one issue, one PR (wire DTOs + endpoints + harness capture extension +
  reconciled fixtures + gated live round-trip test).
- **Builds on:** the read-path wire slice
  ([design](2026-07-10-ibkr-cpapi-readpath-wire-design.md); PRs #127 / #129 / #131).
  Reuses the existing `CpapiError` envelope and `decode<T>` unchanged, and the existing
  `docker/cpapi/` gateway harness.

## 1. Context & motivation

The read-path wire slice landed `oath-adapter-ibkr` with the CP API v1 **read**
endpoints (`auth/status`, `tickle`, accounts, positions, `secdef` search/info), a
`CpapiError` envelope, a `decode<T>` entry point, and the hand-rolled paper gateway
harness. It **deliberately deferred the order write path**, with a documented reason
([read-path spec §2](2026-07-10-ibkr-cpapi-readpath-wire-design.md)):

> Order **write** path (place / the two-step reply-confirm / cancel / modify) — couples
> to the unbuilt order-safety contract (ADR-0022 / ADR-0026), so it would churn.

That caveat is about the **domain-translation / order-safety half** — OATH's
`Order`/`OrderId`, idempotency, and the order-safety contract — **not** the IBKR wire
format. IBKR's CP API v1 order request/response JSON is stable and OATH-agnostic. So a
**pure wire slice** — serde DTOs mirroring IBKR's JSON, no `Order`/`OrderId`, no
idempotency or safety logic — sidesteps the churn in exactly the way the read-path wire
slice sidestepped it for reads. ADR-0022 / ADR-0026 stay untouched.

The OATH-side foundations remain empty skeletons, unchanged since the read-path slice:
[`oath-adapter-api`](../../../crates/adapter/api/src/lib.rs) has **no `Broker` /
`DataProvider` trait**, and [`oath-model`](../../../crates/model/src/) has **no
`InstrumentId` / `Order` / `OrderId`**. This slice touches none of them — it is the
venue-side half of [ADR-0003](../../../docs/adr/0003-canonical-model-adapter-translation.md)'s
anti-corruption boundary, extended from reads to writes.

**The defining new shape of the write path:** it is the crate's *first* `Serialize`
(request-body) direction, and it must model IBKR's **order-reply confirmation dance** —
submitting an order returns either the placement confirmation **or** a list of warning
"questions" that must be echoed back to `POST /iserver/reply/{replyId}` with
`{"confirmed": true}` before the order goes live.

## 2. Scope

**In:**

- **Request-body DTOs** (the crate's first serialize direction):
  - `PlaceOrderRequest { orders: Vec<OrderRequest> }` — the `POST …/orders` body
    (`{"orders":[…]}`).
  - `OrderRequest` — a focused, common field set (see §3.1 / §7.2).
  - `ReplyConfirm { confirmed: bool }` — the `POST /iserver/reply/{id}` body.
- **Response DTOs** for the order lifecycle:
  - `OrderPlaceReply` — the place/reply **union**, one all-optional struct decoded as
    `Vec<OrderPlaceReply>` (§3.1 / §7.3).
  - `CancelResponse`, `OrderStatus`, `LiveOrders` + `LiveOrder`.
- **`Endpoint` descriptors** for place, reply-confirm, cancel, order-status, live-orders
  (§3.3); adds `Method::Delete`.
- **Fixture-based unit tests** (decode + serialize) that run in `just ci`.
- **Harness extension:** `just ibkr-capture` / `docker/cpapi/capture.sh` gains the
  stateful order dance to capture real, sanitized fixtures (§3.2).
- A **gated `#[ignore]` live round-trip test** (place → confirm → cancel).

**Out (deferred on purpose):**

- **Order modify** (`POST …/order/{orderId}`) — YAGNI for this slice; a natural later
  addition once place/cancel/status are proven.
- **Any OATH-domain translation** — no `Order`/`OrderId`, no idempotency/`cOID`
  generation policy, no order-state machine, no order-safety semantics (ADR-0022 /
  ADR-0026 stay untouched). The wire layer *carries* `cOID` as an optional passthrough
  field; it does not *own* order identity.
- **No transport, no auth** — unchanged from the read path.
- **Exotic order features** — bracket / OCA groups, trailing stops, algo params,
  `secType` / `listingExchange` disambiguation. A later slice if needed.
- **WS order-status streaming** — needs a `net-ws` backend that does not exist yet.

## 3. Architecture

An extension of the existing `cpapi` module plus a harness capture-script extension. No
new crate, no new dependencies beyond what the read path already pulls
(`serde`/`serde_json`/`thiserror`).

### 3.1 `cpapi` wire module — new `order.rs`

All order DTOs live in a new `cpapi/order.rs`, re-exported from `cpapi/mod.rs`. One
cohesive module (~6 structs, request + response). If it grows unwieldy, split the
read-back types (`OrderStatus`, `LiveOrders`) into `cpapi/order_status.rs` — but start
unified.

**Request DTOs (first `Serialize` direction):**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOrderRequest { pub orders: Vec<OrderRequest> }   // body: {"orders":[…]}

#[derive(Debug, Clone, Serialize)]
pub struct OrderRequest {
    pub conid: i64,
    pub side: String,                                  // "BUY" / "SELL" — no enum
    #[serde(rename = "orderType")] pub order_type: String,   // "LMT" / "MKT" / …
    pub quantity: serde_json::Number,
    pub tif: String,                                   // "DAY" / "GTC" / …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<serde_json::Number>,             // LMT
    #[serde(rename = "auxPrice", skip_serializing_if = "Option::is_none")]
    pub aux_price: Option<serde_json::Number>,         // STP
    #[serde(rename = "cOID", skip_serializing_if = "Option::is_none")]
    pub coid: Option<String>,                          // customer order id — passthrough
    #[serde(rename = "outsideRTH", skip_serializing_if = "Option::is_none")]
    pub outside_rth: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplyConfirm { pub confirmed: bool }        // body: {"confirmed":true}
```

- **No enums for `side` / `order_type` / `tif`** — kept as `String`, consistent with the
  read path's "no interpretation at the wire layer." Mapping OATH's `Side`/order types
  onto these strings is the deferred translation layer's job.
- **Numbers as `serde_json::Number`** for `quantity` / `price` / `aux_price` — honors
  "no premature `f64`"; the translation layer produces exact decimals from OATH
  fixed-point (ADR-0023).
- **`skip_serializing_if`** on every optional so absent fields don't emit `null`
  (matches how IBKR clients send sparse order bodies).
- **No bespoke `encode()` helper** (§7.1) — request DTOs derive `Serialize`; the future
  transport serializes with `serde_json::to_vec`.

**Response DTOs (all derive `Debug, Clone, Deserialize` only — no `PartialEq`; money/qty
as `serde_json::Number`; anything not guaranteed is `Option`):**

```rust
// The place / reply-confirm union — one all-optional struct (§7.3).
// decode::<Vec<OrderPlaceReply>>(bytes)
pub struct OrderPlaceReply {
    // "question" shape (a suppressible warning to confirm)
    pub id: Option<String>,
    pub message: Option<Vec<String>>,
    #[serde(rename = "isSuppressed")] pub is_suppressed: Option<bool>,
    // "confirmation" shape (order accepted)
    pub order_id: Option<String>,
    pub order_status: Option<String>,
    pub encrypt_message: Option<String>,
}

pub struct CancelResponse {   // fields reconciled from live capture
    pub order_id: Option<i64>, pub msg: Option<String>,
    pub conid: Option<i64>, pub account: Option<String>,
}
pub struct OrderStatus { /* order_id, conid, side, order_status, size fields — from live capture */ }
pub struct LiveOrders { pub orders: Vec<LiveOrder>, pub snapshot: Option<bool> }
pub struct LiveOrder  { /* order_id, conid, ticker, side, status, … — from live capture */ }
```

The exact field lists of `CancelResponse`, `OrderStatus`, and `LiveOrder` are **pinned
by the live capture** (§3.2) — the read-path slice proved that only a real capture
reveals IBKR's quirks (misspelled keys, string-vs-int ids, object-vs-array). The
structs above are the starting shape, reconciled at capture time.

### 3.2 Paper gateway harness — capture extension

Extend `just ibkr-capture` / `docker/cpapi/capture.sh` with the **stateful order dance**
against a logged-in paper gateway (single 5-min session window after manual browser
login, as established for the read path):

1. **Place** a deliberately **far-from-market resting limit** order (a marketable order
   could fill; a far-off limit rests so nothing executes) → capture the response
   (`order_place_questions.json` if IBKR raises a warning, else the confirmation).
2. **Confirm** any reply questions via `POST /iserver/reply/{id}` `{"confirmed":true}` →
   capture (`order_reply_confirmed.json`).
3. **Read back** `GET /iserver/account/order/status/{orderId}` and
   `GET /iserver/account/orders` → capture (`order_status.json`, `live_orders.json`).
4. **Cancel** `DELETE /iserver/account/{acct}/order/{orderId}` → capture
   (`order_cancel.json`).

- **Forcing the questions branch:** IBKR raises confirmable warnings only for certain
  precautions (order value / size caps, price-vs-market), so a plain far-off limit may
  return a confirmation directly with no question. The capture drives a known precaution
  (e.g. an order value / size that trips a suppressible warning) to capture a real
  questions-shape response; if no precaution can be triggered live, the questions fixture
  is authored as a **documented representative** one (its shape is well known and covered
  by the union struct) while the confirmation, status, live-orders, and cancel fixtures
  remain live-captured.
- **Safety:** far-off limit price + immediate cancel so nothing executes; paper account
  only. The capture script asserts the cancel succeeded (leaves no resting order).
- **Sanitization** (documented pass, only sanitized fixtures committed): account →
  `DU0000000`; order ids / `replyId` / `cOID` → stable placeholders; timestamps → fixed
  placeholder. `conid`s kept (public reference data), per the read-path scrub policy.
- Fixtures land in `crates/adapter/ibkr/tests/fixtures/cpapi/`.

### 3.3 `Endpoint` descriptors (added to `cpapi/endpoint.rs`)

Alongside the existing read constructors; adds `Method::Delete` to the `Method` enum.

| Constructor | Method + path |
| --- | --- |
| `place_orders(account_id)` | `POST /iserver/account/{account_id}/orders` |
| `reply(reply_id)` | `POST /iserver/reply/{reply_id}` |
| `cancel_order(account_id, order_id)` | `DELETE /iserver/account/{account_id}/order/{order_id}` |
| `order_status(order_id)` | `GET /iserver/account/order/status/{order_id}` |
| `live_orders()` | `GET /iserver/account/orders` |

Request **bodies** remain out of the `Endpoint` descriptor (method + path only, as the
read path established); the body is the separately-modeled request DTO the future
transport serializes.

## 4. Testing strategy (TDD, fixture-driven)

- **Red → green per DTO:** a test deserializes `tests/fixtures/cpapi/<name>.json` into
  the target type and asserts key fields → fails (type absent) → define the DTO → green.
  Fixtures are **real, sanitized paper-gateway responses** (captured via §3.2).
- **Serialize tests:** assert `OrderRequest` / `PlaceOrderRequest` / `ReplyConfirm`
  produce the exact wire JSON — field renames (`orderType`, `auxPrice`, `cOID`,
  `outsideRTH`), and that absent optionals are omitted (not `null`).
- **Union coverage:** decode both a questions-shape and a confirmation-shape fixture into
  `Vec<OrderPlaceReply>`, asserting the right fields are `Some` / `None` in each.
- **Gated live round-trip test:** `#[ignore]` (needs a live authenticated gateway).
  Drives place → confirm → cancel via `curl -k` and decodes each live response with the
  DTOs. `#[ignore]` keeps it out of `just ci` regardless of `--all-features` (same
  rationale as the read-path live test). Documented run command.
- **CI:** only fixture-based unit tests run in `just ci`; the DoD stays green offline.

## 5. Workspace / lint conformance

- Compiles under `[workspace.lints]`: no `unsafe`, **no `unwrap`/`expect`/indexing** in
  non-test code, `missing_docs` satisfied (every new public item documented), edition
  2024 / MSRV 1.90.
- No new dependencies (`serde`/`serde_json`/`thiserror` already present).
  `cargo-deny` / `typos` / `cargo-machete` / `shellcheck` (capture script) clean.
- Update the [README](../../../README.md) crate-table row for `oath-adapter-ibkr` (read
  path → "read + order write path").

## 6. Deliverables / Definition of done

- `cpapi/order.rs` (request + response DTOs) + order `Endpoint` constructors in
  `cpapi/endpoint.rs`, re-exported from `cpapi/mod.rs`.
- Extended `docker/cpapi/capture.sh` / `just ibkr-capture` running the order dance;
  reconciled, sanitized fixtures committed.
- Fixture decode + serialize tests passing in `just ci`.
- Gated live round-trip test present, excluded from CI.
- README crate-table row updated; `CHANGELOG.md` `[Unreleased]` entry added.
- `just ci` green — **including `just doc`** (broken intra-doc links pass
  check/lint/test but fail doc).

## 7. Decisions (settled 2026-07-11)

1. **Pure-wire boundary — no order-safety coupling.** Model IBKR's order request/response
   JSON only. No OATH `Order`/`OrderId`, no idempotency/`cOID` policy, no order-state
   machine, no ADR-0022/0026 dependency. This is why the write path — deferred by the
   read-path spec as "churn-prone" — is safe to build now: the churn risk was in the
   translation half, which stays out.
2. **No `encode()` helper; focused request field set.** Request DTOs derive `Serialize`;
   the future transport serializes them (`decode` earns its place because response bytes
   need error mapping — outgoing well-typed requests do not). `OrderRequest` carries the
   common fields (`conid`, `side`, `orderType`, `quantity`, `tif`, `price`, `auxPrice`,
   `cOID`, `outsideRTH`); exotic order features are a later slice (YAGNI).
3. **Place/reply union — one all-optional struct, not an untagged enum.** IBKR returns
   *either* warning "questions" *or* order confirmations from both the place and the
   reply endpoints. Model each array element as a single `OrderPlaceReply` carrying both
   shapes' fields as `Option`, decoded as `Vec<OrderPlaceReply>`; the caller inspects
   which fields are present. This matches the read path's faithful-mirror /
   no-interpretation philosophy, is a single `decode`, and avoids serde `untagged`'s
   order-sensitivity and poor error messages.
4. **Live-capture in-slice, not representative-then-reconcile.** Unlike the read path's
   two-PR rhythm (#127 representative → #129 live-reconciled), this slice captures real
   sanitized fixtures in-band by driving the actual order dance, and reconciles the DTOs
   to the live wire immediately — one self-contained PR. Requires a logged-in paper
   gateway and tolerates real paper-order side effects (mitigated by a far-off resting
   limit + immediate cancel).
5. **Modify deferred.** `POST …/order/{orderId}` (modify) is out of scope; place +
   reply-confirm + cancel + status + live-orders is the "submission / cancel / status"
   surface. Modify is a clean later addition.
