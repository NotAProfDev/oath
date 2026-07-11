# IBKR CPAPI Write-Path Wire Layer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `oath-adapter-ibkr` with the Client Portal API v1 (`cpapi`) **order write path** as a pure wire layer — request-body serde DTOs, the order/reply union response, cancel/status/live-orders response DTOs, and endpoint descriptors — plus a harness extension that captures the real place→confirm→cancel dance into sanitized fixtures, and a gated live round-trip test.

**Architecture:** A new `cpapi/order.rs` module holds the crate's first `Serialize` (request) DTOs and the order-lifecycle response DTOs. Order `Endpoint` constructors join the existing read ones in `cpapi/endpoint.rs`. Fixtures are driven TDD-first as documented *representative* JSON, then replaced in-slice with **real, sanitized** captures by extending `docker/cpapi/capture.sh` to drive the stateful order dance against a logged-in paper gateway. No transport, no auth, no OATH-domain translation, no order-safety semantics — ADR-0022/0026 stay untouched.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `serde`/`serde_json` derive, `thiserror`; the existing Docker Client Portal Gateway harness; `just` recipes; `curl` + `python3` for the capture dance.

**Spec:** [docs/superpowers/specs/2026-07-11-ibkr-cpapi-writepath-wire-design.md](../specs/2026-07-11-ibkr-cpapi-writepath-wire-design.md). Builds on the read-path slice ([spec](../specs/2026-07-10-ibkr-cpapi-readpath-wire-design.md) / [plan](2026-07-10-ibkr-cpapi-readpath-wire.md); PRs #127/#129/#131).

**Worktree:** already created at `.claude/worktrees/ibkr-cpapi-writepath` on branch `feat/ibkr-cpapi-writepath` (this plan and the spec are committed there). All work happens in that worktree — never switch the primary checkout's branch.

## Global Constraints

*Every task's requirements implicitly include this section.*

- **Edition 2024, MSRV 1.90.** Validate with `just msrv`.
- **`#![forbid(unsafe_code)]`** is already at the crate root; keep it.
- **`just lint` runs `cargo clippy … -- -D warnings`.** For every `pub` item and `pub` field: a `///` doc comment (`missing_docs`); every type derives `Debug` (`missing_debug_implementations`); constructors returning a value get `#[must_use]` (`must_use_candidate`); public fns returning `Result` get a `/// # Errors` section. **Do not derive `PartialEq` without `Eq`** on an `Eq`-capable type (`derive_partial_eq_without_eq`) — DTOs derive `Debug, Clone, Deserialize` only (add `Serialize` on request DTOs); the endpoint `Method` enum keeps `PartialEq, Eq`.
- **`unwrap`/`expect`/indexing = warn in non-test code**, but **test code is exempt** (`.clippy.toml`). Non-test code returns `Result` / uses `.get()`.
- **No new dependencies.** `serde` (workspace, `features = ["derive"]`), `serde_json`, `thiserror` are already declared. `cargo-machete` runs in CI, so add no unused deps.
- **`serde_json` has NO `arbitrary_precision`** (workspace `serde_json = "1"`, default features). A non-integer `serde_json::Number` round-trips through `f64`, so serialize/`to_string` tests must use **round-trip-safe** literals (`1`, `185.5`) — never trailing-zero decimals like `185.50` (which serialize back as `185.5`).
- **`just test` runs `--all-features`** → a cargo feature cannot exclude a test from CI. Use **`#[ignore]`** for the live test.
- **`just doc` runs `RUSTDOCFLAGS="-D warnings" cargo doc --document-private-items`** → broken intra-doc links fail. Cross-reference DTOs with **backticked names, not `[]` links**. Run `just doc` in every task's verification.
- **`just ci`** = `fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck`. Shell scripts stay shellcheck-clean (`set -euo pipefail`, quoted vars); **no secrets** in committed files (gitleaks); committed fixtures are **sanitized** (no real account ids / order ids / names). *(`typos` was verified to not flag `tif`/`coid`/`rth`/`order_type` etc. — no `_typos.toml` change is needed.)*
- **Faithful-mirror rule (spec §7):** model each field as the wire actually sends it. Number-sent-as-string → `String`; ids/counts → `i64`; precision-sensitive numbers → `serde_json::Number`; **no** OATH-domain translation. `side`/`orderType`/`tif` stay `String` (no enums). Every response field not guaranteed present is `Option`.
- **Provisional-then-real fixtures (spec §7.4):** Tasks 2–4 ship *representative* fixtures to drive TDD; **Task 6 replaces them with real, sanitized captures** and reconciles the DTOs to the live wire. Field spellings/types marked "provisional" below are the reconcile targets.
- **Workflow:** one issue, one PR (this slice). Add a `CHANGELOG.md` `[Unreleased]` entry. `just ci` must pass before the PR.

## File Structure

```
crates/adapter/ibkr/
  src/cpapi/
    endpoint.rs                        # MODIFY: + Method::Delete, + 5 order Endpoint constructors
    order.rs                           # CREATE: request DTOs (Serialize) + order-lifecycle response DTOs
    mod.rs                             # MODIFY: `pub mod order;` + re-exports; broaden module doc
  tests/
    endpoint.rs                        # MODIFY: + order endpoint path tests
    order.rs                           # CREATE: serialize tests + union/cancel/status/live decode tests
    live.rs                            # MODIFY: + gated #[ignore] place->confirm->cancel round-trip
    fixtures/cpapi/
      order_place_questions.json       # CREATE (representative -> real)
      order_reply_confirmed.json       # CREATE (representative -> real)
      order_cancel.json                # CREATE (representative -> real)
      order_status.json                # CREATE (representative -> real)
      live_orders.json                 # CREATE (representative -> real)

docker/cpapi/
  capture.sh                           # MODIFY: + stateful order dance (place/reply/status/live/cancel)
  README.md                            # MODIFY: + order-capture + safety + sanitize notes

README.md                              # MODIFY: crate-table row + coming-soon line (read -> read + write)
CHANGELOG.md                           # MODIFY: [Unreleased] ### Added entry
```

---

### Task 1: `Method::Delete` + order `Endpoint` constructors

**Files:**
- Modify: `crates/adapter/ibkr/src/cpapi/endpoint.rs`
- Test: `crates/adapter/ibkr/tests/endpoint.rs`

**Interfaces:**
- Consumes: existing `Method`, `Endpoint` (read path).
- Produces: `Method::Delete`; constructors `Endpoint::place_orders(account_id: &str)`, `reply(reply_id: &str)`, `cancel_order(account_id: &str, order_id: &str)`, `order_status(order_id: &str)`, `live_orders()`. Paths are relative to the `…/v1/api` base. All id params are `&str` (path interpolation only).

- [ ] **Step 1: Add the failing tests**

Append to `crates/adapter/ibkr/tests/endpoint.rs`:

```rust
#[test]
fn place_orders_is_a_post_with_account_in_path() {
    let ep = Endpoint::place_orders("DU0000000");
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/iserver/account/DU0000000/orders");
}

#[test]
fn reply_interpolates_the_reply_id() {
    let ep = Endpoint::reply("a1b2c3d4-0000");
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/iserver/reply/a1b2c3d4-0000");
}

#[test]
fn cancel_order_is_a_delete() {
    let ep = Endpoint::cancel_order("DU0000000", "1234567890");
    assert_eq!(ep.method, Method::Delete);
    assert_eq!(ep.path, "/iserver/account/DU0000000/order/1234567890");
}

#[test]
fn order_status_interpolates_the_order_id() {
    let ep = Endpoint::order_status("1234567890");
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/account/order/status/1234567890");
}

#[test]
fn live_orders_is_a_get() {
    let ep = Endpoint::live_orders();
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/account/orders");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oath-adapter-ibkr --test endpoint`
Expected: FAIL — `Method::Delete` / `place_orders` / `reply` / `cancel_order` / `order_status` / `live_orders` not found.

- [ ] **Step 3: Add `Delete` to the `Method` enum**

In `crates/adapter/ibkr/src/cpapi/endpoint.rs`, extend `Method` (keep the existing derives and `Get`/`Post`):

```rust
    /// HTTP `POST`.
    Post,
    /// HTTP `DELETE`.
    Delete,
```

- [ ] **Step 4: Add the order constructors**

In `endpoint.rs`, inside `impl Endpoint`, after `secdef_info`:

```rust
    /// `POST /iserver/account/{account_id}/orders` — submit one or more orders.
    /// The body (a `PlaceOrderRequest`) is supplied by the transport, not this descriptor.
    #[must_use]
    pub fn place_orders(account_id: &str) -> Self {
        Self {
            method: Method::Post,
            path: format!("/iserver/account/{account_id}/orders"),
        }
    }

    /// `POST /iserver/reply/{reply_id}` — confirm a suppressible order warning
    /// (body `{"confirmed":true}`, a `ReplyConfirm`).
    #[must_use]
    pub fn reply(reply_id: &str) -> Self {
        Self {
            method: Method::Post,
            path: format!("/iserver/reply/{reply_id}"),
        }
    }

    /// `DELETE /iserver/account/{account_id}/order/{order_id}` — cancel a live order.
    #[must_use]
    pub fn cancel_order(account_id: &str, order_id: &str) -> Self {
        Self {
            method: Method::Delete,
            path: format!("/iserver/account/{account_id}/order/{order_id}"),
        }
    }

    /// `GET /iserver/account/order/status/{order_id}` — status of a single order.
    #[must_use]
    pub fn order_status(order_id: &str) -> Self {
        Self {
            method: Method::Get,
            path: format!("/iserver/account/order/status/{order_id}"),
        }
    }

    /// `GET /iserver/account/orders` — the account's live orders.
    #[must_use]
    pub fn live_orders() -> Self {
        Self {
            method: Method::Get,
            path: "/iserver/account/orders".to_owned(),
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p oath-adapter-ibkr --test endpoint`
Expected: PASS (existing read tests + the 5 new order tests).

- [ ] **Step 6: Lint + doc**

Run: `just lint && just doc`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr/src/cpapi/endpoint.rs crates/adapter/ibkr/tests/endpoint.rs
git commit -m "feat(ibkr): cpapi order Endpoint constructors + Method::Delete"
```

---

### Task 2: Request DTOs — `OrderRequest`, `PlaceOrderRequest`, `ReplyConfirm`

The crate's first `Serialize` (request-body) direction.

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/order.rs`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/order.rs`

**Interfaces:**
- Produces: `OrderRequest`, `PlaceOrderRequest { orders: Vec<OrderRequest> }`, `ReplyConfirm { confirmed: bool }`. Response DTOs are added to the same file in Tasks 3–4.

- [ ] **Step 1: Write the failing serialize tests**

`crates/adapter/ibkr/tests/order.rs`:

```rust
//! Tests for the cpapi order write-path DTOs (request serialize + response decode).
use oath_adapter_ibkr::cpapi::{OrderRequest, PlaceOrderRequest, ReplyConfirm};

#[test]
fn order_request_serializes_with_renames_and_omits_absent_optionals() {
    // Round-trip-safe numbers only: serde_json has no arbitrary_precision, so a
    // trailing-zero decimal like 185.50 would serialize back as 185.5.
    let req = OrderRequest {
        conid: 265_598,
        side: "BUY".to_owned(),
        order_type: "LMT".to_owned(),
        quantity: serde_json::Number::from(1_u64),
        tif: "DAY".to_owned(),
        price: Some("185.5".parse().expect("valid number")),
        aux_price: None,
        coid: Some("oath-0001".to_owned()),
        outside_rth: Some(false),
    };
    let json = serde_json::to_string(&req).expect("serializes");
    assert_eq!(
        json,
        r#"{"conid":265598,"side":"BUY","orderType":"LMT","quantity":1,"tif":"DAY","price":185.5,"cOID":"oath-0001","outsideRTH":false}"#
    );
}

#[test]
fn place_order_request_wraps_orders_array() {
    let req = PlaceOrderRequest {
        orders: vec![OrderRequest {
            conid: 265_598,
            side: "SELL".to_owned(),
            order_type: "MKT".to_owned(),
            quantity: serde_json::Number::from(2_u64),
            tif: "DAY".to_owned(),
            price: None,
            aux_price: None,
            coid: None,
            outside_rth: None,
        }],
    };
    let json = serde_json::to_string(&req).expect("serializes");
    assert_eq!(
        json,
        r#"{"orders":[{"conid":265598,"side":"SELL","orderType":"MKT","quantity":2,"tif":"DAY"}]}"#
    );
}

#[test]
fn reply_confirm_serializes() {
    let json = serde_json::to_string(&ReplyConfirm { confirmed: true }).expect("serializes");
    assert_eq!(json, r#"{"confirmed":true}"#);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oath-adapter-ibkr --test order`
Expected: FAIL — `OrderRequest` / `PlaceOrderRequest` / `ReplyConfirm` not found.

- [ ] **Step 3: Create `order.rs` with the request DTOs**

`crates/adapter/ibkr/src/cpapi/order.rs`:

```rust
//! Order write-path wire layer: request bodies for placing and confirming orders,
//! plus the order-lifecycle response DTOs (place/reply union, cancel, status, live
//! orders). Faithfully mirrors Client Portal API v1 JSON — no transport, no auth, no
//! OATH-domain translation, no order-safety semantics.
//!
//! `side`, `orderType`, and `tif` are kept as `String` (the wire's own tokens); the
//! mapping onto OATH's domain types is the deferred translation layer's job.

use serde::{Deserialize, Serialize};

/// One order in a `POST /iserver/account/{account}/orders` request body.
///
/// A focused subset of IBKR's order fields — enough for common equity orders. Exotic
/// features (bracket / OCA groups, trailing stops, algo params) are a later slice.
/// `quantity` / `price` / `aux_price` are `serde_json::Number` (no premature `f64`);
/// the translation layer produces exact values from OATH fixed-point (ADR-0023).
#[derive(Debug, Clone, Serialize)]
pub struct OrderRequest {
    /// IBKR contract id.
    pub conid: i64,
    /// Order side — `"BUY"` or `"SELL"` (the wire's own token; no enum here).
    pub side: String,
    /// Order type — `"LMT"`, `"MKT"`, `"STP"`, ….
    #[serde(rename = "orderType")]
    pub order_type: String,
    /// Order quantity.
    pub quantity: serde_json::Number,
    /// Time in force — `"DAY"`, `"GTC"`, ….
    pub tif: String,
    /// Limit price (for `LMT`), when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<serde_json::Number>,
    /// Stop / auxiliary price (for `STP`), when applicable.
    #[serde(rename = "auxPrice", skip_serializing_if = "Option::is_none")]
    pub aux_price: Option<serde_json::Number>,
    /// Customer order id (`cOID`) — a client-supplied idempotency tag. Carried through
    /// verbatim; this layer does not generate or interpret it.
    #[serde(rename = "cOID", skip_serializing_if = "Option::is_none")]
    pub coid: Option<String>,
    /// Allow execution outside regular trading hours, when set.
    #[serde(rename = "outsideRTH", skip_serializing_if = "Option::is_none")]
    pub outside_rth: Option<bool>,
}

/// Body of `POST /iserver/account/{account}/orders` — a batch of orders.
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOrderRequest {
    /// The orders to submit.
    pub orders: Vec<OrderRequest>,
}

/// Body of `POST /iserver/reply/{reply_id}` — confirm (or decline) a suppressible
/// order warning.
#[derive(Debug, Clone, Serialize)]
pub struct ReplyConfirm {
    /// `true` to confirm the warning and proceed.
    pub confirmed: bool,
}
```

- [ ] **Step 4: Wire the module + re-exports and broaden the module doc**

In `crates/adapter/ibkr/src/cpapi/mod.rs`:

Change the first doc line from:

```rust
//! Client Portal API v1 (`cpapi`) read-path wire layer: endpoint descriptors and
```

to:

```rust
//! Client Portal API v1 (`cpapi`) wire layer: endpoint descriptors and serde DTOs for
//! the read path and the order write path. Endpoint descriptors and
```

Add `pub mod order;` after `pub mod endpoint;`, and add the re-export (response types join it in Tasks 3–4):

```rust
pub use order::{OrderRequest, PlaceOrderRequest, ReplyConfirm};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p oath-adapter-ibkr --test order`
Expected: PASS (3 serialize tests).

- [ ] **Step 6: Lint + doc**

Run: `just lint && just doc`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr/src/cpapi crates/adapter/ibkr/tests/order.rs
git commit -m "feat(ibkr): cpapi order request DTOs (first serialize direction)"
```

---

### Task 3: Place/reply union response — `OrderPlaceReply`

**Files:**
- Modify: `crates/adapter/ibkr/src/cpapi/order.rs`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Modify: `crates/adapter/ibkr/tests/order.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/order_place_questions.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/order_reply_confirmed.json`

**Interfaces:**
- Consumes: `decode` (read-path Task 3).
- Produces: `OrderPlaceReply` — one all-optional struct decoded as `Vec<OrderPlaceReply>`, carrying **both** the "question" shape (`id`, `message`, `is_suppressed`) and the "confirmation" shape (`order_id`, `order_status`, `encrypt_message`). *(Provisional field spellings/types — reconciled in Task 6. Note `order_id` is a **string** on the place/reply endpoints; contrast the **integer** `order_id` on status/live in Task 4 — a faithful-mirror split like the read path's `conid`.)*

- [ ] **Step 1: Create the representative fixtures**

`crates/adapter/ibkr/tests/fixtures/cpapi/order_place_questions.json`:

```json
[{"id":"a1b2c3d4-0000-0000-0000-000000000000","message":["You are submitting an order without market data. Are you sure you want to submit this order?"],"isSuppressed":false}]
```

`crates/adapter/ibkr/tests/fixtures/cpapi/order_reply_confirmed.json`:

```json
[{"order_id":"1234567890","order_status":"PreSubmitted","encrypt_message":"1"}]
```

- [ ] **Step 2: Add the failing decode tests**

Append to `crates/adapter/ibkr/tests/order.rs` (add `decode, OrderPlaceReply` to the `use` line — final import list becomes `{OrderPlaceReply, OrderRequest, PlaceOrderRequest, ReplyConfirm, decode}`):

```rust
#[test]
fn order_place_questions_decode_as_question_shape() {
    use oath_adapter_ibkr::cpapi::decode;
    let replies: Vec<OrderPlaceReply> =
        decode(include_bytes!("fixtures/cpapi/order_place_questions.json")).expect("questions decode");
    let q = replies.first().expect("one reply");
    assert_eq!(q.id.as_deref(), Some("a1b2c3d4-0000-0000-0000-000000000000"));
    assert_eq!(q.message.as_ref().map(Vec::len), Some(1));
    assert_eq!(q.is_suppressed, Some(false));
    // Confirmation fields are absent on a question.
    assert!(q.order_id.is_none());
    assert!(q.order_status.is_none());
}

#[test]
fn order_reply_confirmed_decodes_as_confirmation_shape() {
    use oath_adapter_ibkr::cpapi::decode;
    let replies: Vec<OrderPlaceReply> =
        decode(include_bytes!("fixtures/cpapi/order_reply_confirmed.json")).expect("confirmation decode");
    let c = replies.first().expect("one reply");
    assert_eq!(c.order_id.as_deref(), Some("1234567890"));
    assert_eq!(c.order_status.as_deref(), Some("PreSubmitted"));
    // Question fields are absent on a confirmation.
    assert!(c.id.is_none());
    assert!(c.message.is_none());
}
```

Also add `OrderPlaceReply` to the top-of-file `use oath_adapter_ibkr::cpapi::{…}` import (keep the existing request-DTO names).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p oath-adapter-ibkr --test order order_place`
Expected: FAIL — `OrderPlaceReply` not found.

- [ ] **Step 4: Implement `OrderPlaceReply` in `order.rs`**

Append to `crates/adapter/ibkr/src/cpapi/order.rs`:

```rust
/// One element of a place-order or reply-confirm response.
///
/// The Client Portal API returns *either* a list of suppressible warning **questions**
/// (confirm each via `POST /iserver/reply/{id}`) *or* a list of order **confirmations**
/// — from both `POST …/orders` and `POST /iserver/reply/{id}`. Rather than a serde
/// `untagged` enum (order-sensitive, poor errors), this is one all-optional struct
/// carrying both shapes; the caller inspects which fields are present. `decode` it as
/// `Vec<OrderPlaceReply>`.
///
/// `order_id` is a **string** here; on `order/status` and `account/orders` the same
/// logical id arrives as an **integer** (`OrderStatus`, `LiveOrder`) — the faithful
/// mirror keeps each as the wire sends it.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderPlaceReply {
    /// Question id to echo back to `POST /iserver/reply/{id}` (question shape).
    pub id: Option<String>,
    /// Human-readable warning lines (question shape).
    pub message: Option<Vec<String>>,
    /// Whether this warning can be suppressed (question shape).
    #[serde(rename = "isSuppressed")]
    pub is_suppressed: Option<bool>,
    /// Placed order id, as a string (confirmation shape).
    pub order_id: Option<String>,
    /// Order status, e.g. `"PreSubmitted"` (confirmation shape).
    pub order_status: Option<String>,
    /// Opaque encrypt-message token IBKR echoes on confirmation (confirmation shape).
    pub encrypt_message: Option<String>,
}
```

- [ ] **Step 5: Extend the re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, extend the order re-export:

```rust
pub use order::{OrderPlaceReply, OrderRequest, PlaceOrderRequest, ReplyConfirm};
```

- [ ] **Step 6: Run tests, lint, doc**

Run: `cargo test -p oath-adapter-ibkr --test order` → PASS.
Run: `just lint && just doc` → clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi OrderPlaceReply union DTO + fixtures"
```

---

### Task 4: Cancel + status + live-orders response DTOs

**Files:**
- Modify: `crates/adapter/ibkr/src/cpapi/order.rs`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Modify: `crates/adapter/ibkr/tests/order.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/order_cancel.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/order_status.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/live_orders.json`

**Interfaces:**
- Produces: `CancelResponse`, `OrderStatus`, `LiveOrders { orders: Vec<LiveOrder>, snapshot: Option<bool> }`, `LiveOrder`. *(Provisional — reconciled in Task 6.)* The **camel-vs-snake split** is faithful to IBKR: `order/status` is snake-native (`order_id`, `order_type`, `order_status` — no renames), while `account/orders` is camel-native (`orderId`, `orderType`, `totalSize` — renamed). `order_id` is an **integer** on all three (contrast the string on place/reply, Task 3).

- [ ] **Step 1: Create the representative fixtures**

`crates/adapter/ibkr/tests/fixtures/cpapi/order_cancel.json`:

```json
{"order_id":1234567890,"msg":"Request was submitted","conid":265598,"account":"DU0000000"}
```

`crates/adapter/ibkr/tests/fixtures/cpapi/order_status.json`:

```json
{"order_id":1234567890,"conid":265598,"symbol":"AAPL","side":"B","order_type":"Limit","order_status":"PreSubmitted","total_size":"1","cum_fill":"0","price":"1.00","tif":"DAY"}
```

`crates/adapter/ibkr/tests/fixtures/cpapi/live_orders.json`:

```json
{"orders":[{"acct":"DU0000000","conid":265598,"orderId":1234567890,"ticker":"AAPL","side":"BUY","status":"PreSubmitted","orderType":"LIMIT","totalSize":1,"price":1.5}],"snapshot":false}
```

- [ ] **Step 2: Add the failing decode tests**

Append to `crates/adapter/ibkr/tests/order.rs` (extend the top import to add `CancelResponse, LiveOrders, OrderStatus`):

```rust
#[test]
fn cancel_response_decodes() {
    use oath_adapter_ibkr::cpapi::decode;
    let resp: CancelResponse =
        decode(include_bytes!("fixtures/cpapi/order_cancel.json")).expect("cancel decodes");
    assert_eq!(resp.order_id, Some(1_234_567_890));
    assert_eq!(resp.msg.as_deref(), Some("Request was submitted"));
}

#[test]
fn order_status_decodes_snake_fields() {
    use oath_adapter_ibkr::cpapi::decode;
    let status: OrderStatus =
        decode(include_bytes!("fixtures/cpapi/order_status.json")).expect("status decodes");
    assert_eq!(status.order_id, Some(1_234_567_890));
    assert_eq!(status.order_status.as_deref(), Some("PreSubmitted"));
    // Sizes arrive as strings on this endpoint — kept faithfully as String.
    assert_eq!(status.total_size.as_deref(), Some("1"));
}

#[test]
fn live_orders_decode_camel_fields() {
    use oath_adapter_ibkr::cpapi::decode;
    let live: LiveOrders =
        decode(include_bytes!("fixtures/cpapi/live_orders.json")).expect("live orders decode");
    assert_eq!(live.snapshot, Some(false));
    let o = live.orders.first().expect("one live order");
    assert_eq!(o.order_id, Some(1_234_567_890)); // renamed from "orderId"
    assert_eq!(o.ticker.as_deref(), Some("AAPL"));
    assert_eq!(o.order_type.as_deref(), Some("LIMIT")); // renamed from "orderType"
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p oath-adapter-ibkr --test order`
Expected: FAIL — `CancelResponse` / `OrderStatus` / `LiveOrders` not found.

- [ ] **Step 4: Implement the response DTOs in `order.rs`**

Append to `crates/adapter/ibkr/src/cpapi/order.rs`:

```rust
/// Response of `DELETE /iserver/account/{account}/order/{order_id}` — cancel ack.
#[derive(Debug, Clone, Deserialize)]
pub struct CancelResponse {
    /// The cancelled order id (integer on this endpoint).
    pub order_id: Option<i64>,
    /// Human-readable acknowledgement, e.g. `"Request was submitted"`.
    pub msg: Option<String>,
    /// Contract id, when present.
    pub conid: Option<i64>,
    /// Account id, when present.
    pub account: Option<String>,
}

/// Response of `GET /iserver/account/order/status/{order_id}`.
///
/// This endpoint is **snake_case**-native — no serde renames. Sizes arrive as strings
/// (kept faithfully as `String`); `price` is a bare `serde_json::Number`.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderStatus {
    /// Order id (integer on this endpoint).
    pub order_id: Option<i64>,
    /// Contract id, when present.
    pub conid: Option<i64>,
    /// Symbol, when present.
    pub symbol: Option<String>,
    /// Side token (e.g. `"B"`/`"S"`), when present.
    pub side: Option<String>,
    /// Order type token, when present.
    pub order_type: Option<String>,
    /// Order status, when present.
    pub order_status: Option<String>,
    /// Total order size, sent as a string.
    pub total_size: Option<String>,
    /// Cumulative filled size, sent as a string.
    pub cum_fill: Option<String>,
    /// Limit/last price, when present.
    pub price: Option<serde_json::Number>,
    /// Time in force, when present.
    pub tif: Option<String>,
}

/// Response of `GET /iserver/account/orders` — the account's live orders.
///
/// This endpoint is **camelCase**-native, so each `LiveOrder` renames `orderId` /
/// `orderType` / `totalSize` — contrast the snake-native `OrderStatus`.
#[derive(Debug, Clone, Deserialize)]
pub struct LiveOrders {
    /// The live orders (may be empty; a first call can return a warming snapshot).
    #[serde(default)]
    pub orders: Vec<LiveOrder>,
    /// Whether this is a pre-warm snapshot rather than live data, when present.
    pub snapshot: Option<bool>,
}

/// One element of a `LiveOrders` response.
#[derive(Debug, Clone, Deserialize)]
pub struct LiveOrder {
    /// Account id, when present.
    pub acct: Option<String>,
    /// Contract id, when present.
    pub conid: Option<i64>,
    /// Order id (integer; camelCase `orderId` on this endpoint).
    #[serde(rename = "orderId")]
    pub order_id: Option<i64>,
    /// Ticker symbol, when present.
    pub ticker: Option<String>,
    /// Side token, when present.
    pub side: Option<String>,
    /// Order status, when present.
    pub status: Option<String>,
    /// Order type (camelCase `orderType` on this endpoint), when present.
    #[serde(rename = "orderType")]
    pub order_type: Option<String>,
    /// Total order size (camelCase `totalSize`), when present.
    #[serde(rename = "totalSize")]
    pub total_size: Option<serde_json::Number>,
    /// Limit/last price, when present.
    pub price: Option<serde_json::Number>,
}
```

- [ ] **Step 5: Extend the re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, extend the order re-export:

```rust
pub use order::{
    CancelResponse, LiveOrder, LiveOrders, OrderPlaceReply, OrderRequest, OrderStatus,
    PlaceOrderRequest, ReplyConfirm,
};
```

- [ ] **Step 6: Run tests, lint, doc**

Run: `cargo test -p oath-adapter-ibkr --test order` → PASS (all order tests).
Run: `just lint && just doc` → clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi cancel/status/live-orders response DTOs + fixtures"
```

---

### Task 5: Harness capture extension — the order dance

**Files:**
- Modify: `docker/cpapi/capture.sh`
- Modify: `docker/cpapi/README.md`

**Interfaces:**
- Produces: `just ibkr-capture <account>` additionally drives place → (reply-confirm loop) → status → live-orders → cancel and writes the five order fixtures. Empirical infra — verified by shellcheck + a live run (Task 6), not a unit test. No `Justfile` change (the recipe already forwards the account arg).

- [ ] **Step 1: Append the order dance to `capture.sh`**

At the end of `docker/cpapi/capture.sh` (before the final `echo "DONE…"` line — keep that line last), insert:

```bash
# ---- Order write-path capture (paper account only; has real side effects) ----
# Places a deliberately far-below-market resting LIMIT BUY so it will NOT fill, drives
# the reply-confirm dance, reads status + live orders, then cancels. Override the
# contract with IBKR_CONID (default AAPL 265598).
if [ -n "$ACCOUNT" ]; then
  CONID="${IBKR_CONID:-265598}"
  # jget FILE KEY -> prints top-level element [0][KEY] of a JSON array, else "".
  jget() {
    python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
print(d[0].get(sys.argv[2],"") if isinstance(d,list) and d else "")' "$1" "$2"
  }

  curl -fksS --max-time 30 -X POST "$BASE/iserver/account/$ACCOUNT/orders" \
    -H 'Content-Type: application/json' \
    -d "{\"orders\":[{\"conid\":$CONID,\"orderType\":\"LMT\",\"side\":\"BUY\",\"quantity\":1,\"tif\":\"DAY\",\"price\":1.00,\"outsideRTH\":false}]}" \
    -o "$OUT/order_place.json"
  echo "captured order_place.json"

  reply_id=$(jget "$OUT/order_place.json" id)
  if [ -n "$reply_id" ]; then
    cp "$OUT/order_place.json" "$OUT/order_place_questions.json"
    for _ in 1 2 3 4 5; do
      curl -fksS --max-time 30 -X POST "$BASE/iserver/reply/$reply_id" \
        -H 'Content-Type: application/json' -d '{"confirmed":true}' \
        -o "$OUT/order_reply_confirmed.json"
      echo "captured order_reply_confirmed.json (reply $reply_id)"
      reply_id=$(jget "$OUT/order_reply_confirmed.json" id)
      if [ -z "$reply_id" ]; then break; fi
    done
    confirm_file="$OUT/order_reply_confirmed.json"
  else
    echo "no reply question was raised; order_place.json IS the confirmation."
    echo "  -> author order_place_questions.json as a documented representative fixture."
    confirm_file="$OUT/order_place.json"
  fi

  order_id=$(jget "$confirm_file" order_id)
  if [ -n "$order_id" ]; then
    fetch GET "/iserver/account/order/status/$order_id" order_status.json
    fetch GET "/iserver/account/orders" live_orders.json
    curl -fksS --max-time 30 -X DELETE "$BASE/iserver/account/$ACCOUNT/order/$order_id" \
      -o "$OUT/order_cancel.json"
    echo "captured order_cancel.json (cancelled order $order_id)"
    rm -f "$OUT/order_place.json"
  else
    echo "WARNING: no order_id parsed; skipping status/live/cancel. Inspect order_place/reply output."
  fi
else
  echo "skipping order write-path capture: pass an account id (arg 1 or IBKR_ACCOUNT)"
fi
```

*(`order_place.json` is an intermediate scratch file — removed once its confirmation is captured. Only the five committed fixtures remain.)*

- [ ] **Step 2: Update the harness README**

In `docker/cpapi/README.md`, under the `## Capture fixtures` section (after the existing "This writes raw JSON…" paragraph), add:

````markdown

### Order write-path capture (paper only — has side effects)

`just ibkr-capture <account>` also drives the order lifecycle: it places a **far-below-
market resting LIMIT BUY** (price `1.00`, so it will not fill), confirms any reply
warning, reads the order status + live orders, then **cancels** it. Nothing executes,
but it does place and cancel a real paper order. Override the contract with
`IBKR_CONID` (default AAPL `265598`).

If the gateway raises no confirmable warning, `order_place.json` is the confirmation
directly and no `order_place_questions.json` is captured — author that one as a
documented representative fixture (its shape is covered by `OrderPlaceReply`).
````

And extend the `## Sanitize before committing (required)` list with an order-fixture bullet:

```markdown
- in the order fixtures, replace order ids (`order_id` / `orderId`) with a placeholder
  like `1234567890` and any reply `id` with a fixed UUID placeholder.
```

- [ ] **Step 3: Verify shellcheck + the recipe still lists**

Run: `shellcheck docker/cpapi/capture.sh`
Expected: no findings.

Run: `just --list | grep ibkr-capture`
Expected: the recipe is listed (unchanged).

- [ ] **Step 4: Commit**

```bash
git add docker/cpapi/capture.sh docker/cpapi/README.md
git commit -m "feat(ibkr): capture.sh order dance (place/confirm/status/live/cancel)"
```

---

### Task 6: Capture & commit real paper-account order fixtures

**Files:**
- Modify: `crates/adapter/ibkr/tests/fixtures/cpapi/order_*.json`, `live_orders.json` (representative → real, sanitized)
- Modify (as needed): `crates/adapter/ibkr/src/cpapi/order.rs` + `crates/adapter/ibkr/tests/order.rs` to reconcile with real fields

**Interfaces:**
- Consumes: the harness (Task 5) and the DTOs (Tasks 2–4).
- Produces: **real, sanitized** order fixtures the tests pass against — satisfying the spec's live-capture-in-slice DoD, and DTOs reconciled to the live wire.

> **Human-gated:** needs a logged-in paper gateway and tolerates a real (immediately-cancelled, non-filling) paper order. Do this before merging; it replaces the representative fixtures from Tasks 3–4 with reality and reconciles any DTO field differences (the tests are your guide).

- [ ] **Step 1: Bring up the gateway and log in**

Run: `docker compose -f docker/cpapi/docker-compose.yml up -d --build`, then log in with paper credentials at `https://localhost:5000` (see `docker/cpapi/README.md`). Confirm `docker ps` shows the container `healthy`.

- [ ] **Step 2: Capture the order dance**

Run: `just ibkr-capture <YOUR_PAPER_ACCOUNT_ID>`
Expected: the read fixtures refresh, and `order_place_questions.json` (if a warning was raised), `order_reply_confirmed.json`, `order_status.json`, `live_orders.json`, `order_cancel.json` are written. The script cancels the order (verify no resting order remains in the gateway UI / `live_orders.json`).

- [ ] **Step 3: Sanitize**

Edit each order fixture: replace account ids with `DU0000000`, order ids (`order_id`/`orderId`) with `1234567890`, any reply `id` with `a1b2c3d4-0000-0000-0000-000000000000`, and zero quantities/prices. Keep `conid`s.

If no `order_place_questions.json` was captured (no warning raised), keep the representative one from Task 3 (documented as such).

Run (JSON still valid): `for f in crates/adapter/ibkr/tests/fixtures/cpapi/order_*.json crates/adapter/ibkr/tests/fixtures/cpapi/live_orders.json; do python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f"; done`
Expected: no output (all valid).

- [ ] **Step 4: Run the fixture tests against real data; reconcile**

Run: `cargo test -p oath-adapter-ibkr --test order`
Expected: PASS. If a real field differs from the representative shape (a `#[serde(rename)]` needed, an `Option`, a string-vs-int type, the string-vs-int `order_id` split, or the camel-vs-snake split), adjust the DTO in `src/cpapi/order.rs` and the assertion in `tests/order.rs`, honoring the faithful-mirror rule. Re-run until green.

- [ ] **Step 5: Guard against leaked ids**

Run: `git grep -nE '\b(DU|U)[0-9]{6,7}\b' -- crates/adapter/ibkr/tests/fixtures | grep -v 'DU0000000' || echo "clean"`
Expected: `clean` (only the `DU0000000` placeholder remains).

- [ ] **Step 6: Lint, doc, commit**

Run: `just lint && just doc` → clean.

```bash
git add crates/adapter/ibkr
git commit -m "test(ibkr): real sanitized order-lifecycle fixtures + DTO reconcile"
```

---

### Task 7: Gated live round-trip test

**Files:**
- Modify: `crates/adapter/ibkr/tests/live.rs`

**Interfaces:**
- Consumes: `decode`, `OrderPlaceReply`, `CancelResponse`, `Endpoint`. Shells `curl -fksS` at the running gateway; `#[ignore]`d so it stays out of `just ci`.

- [ ] **Step 1: Add the ignored round-trip test**

Append to `crates/adapter/ibkr/tests/live.rs` (extend the existing `use oath_adapter_ibkr::cpapi::{AuthStatus, decode};` import to add `CancelResponse, OrderPlaceReply`):

```rust
/// Drives a full place -> confirm -> cancel round-trip against a live gateway.
/// Places a far-below-market resting LIMIT BUY (will not fill), confirms any reply
/// warning, then cancels. Requires `IBKR_ACCOUNT` (paper). Override the contract with
/// `IBKR_CONID` (default AAPL 265598).
#[test]
#[ignore = "requires a live, authenticated Client Portal Gateway + IBKR_ACCOUNT (paper)"]
fn live_order_place_confirm_cancel_round_trip() {
    let base = std::env::var("IBKR_GATEWAY")
        .unwrap_or_else(|_| "https://localhost:5000/v1/api".to_owned());
    let account = std::env::var("IBKR_ACCOUNT").expect("set IBKR_ACCOUNT to a paper account id");
    let conid = std::env::var("IBKR_CONID").unwrap_or_else(|_| "265598".to_owned());

    let curl = |args: &[&str]| -> Vec<u8> {
        let output = Command::new("curl")
            .args(["-fksS", "--max-time", "30"])
            .args(args)
            .output()
            .expect("curl should run");
        assert!(output.status.success(), "curl failed: {output:?}");
        output.stdout
    };

    // Place.
    let body = format!(
        r#"{{"orders":[{{"conid":{conid},"orderType":"LMT","side":"BUY","quantity":1,"tif":"DAY","price":1.00,"outsideRTH":false}}]}}"#
    );
    let placed = curl(&[
        "-X", "POST", &format!("{base}/iserver/account/{account}/orders"),
        "-H", "Content-Type: application/json", "-d", &body,
    ]);
    let mut replies: Vec<OrderPlaceReply> = decode(&placed).expect("place decodes");

    // Confirm the reply chain until an order_id appears (bounded).
    for _ in 0..5 {
        let Some(reply_id) = replies.first().and_then(|r| r.id.clone()) else { break };
        let confirmed = curl(&[
            "-X", "POST", &format!("{base}/iserver/reply/{reply_id}"),
            "-H", "Content-Type: application/json", "-d", r#"{"confirmed":true}"#,
        ]);
        replies = decode(&confirmed).expect("reply decodes");
    }
    let order_id = replies
        .first()
        .and_then(|r| r.order_id.clone())
        .expect("a confirmed order_id");

    // Cancel — always, so the round-trip leaves no resting order.
    let cancelled = curl(&[
        "-X", "DELETE",
        &format!("{base}/iserver/account/{account}/order/{order_id}"),
    ]);
    let _resp: CancelResponse = decode(&cancelled).expect("cancel decodes");
}
```

- [ ] **Step 2: Verify it compiles and is skipped by default**

Run: `cargo test -p oath-adapter-ibkr --test live`
Expected: compiles; both live tests report `ignored` (0 run).

Run: `just lint`
Expected: clean.

- [ ] **Step 3: (Optional, manual) run it against the live gateway**

With the gateway up + logged in: `IBKR_ACCOUNT=<paper-acct> cargo test -p oath-adapter-ibkr --test live -- --ignored`
Expected: PASS (places + cancels a paper order).

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/ibkr/tests/live.rs
git commit -m "test(ibkr): gated live place/confirm/cancel round-trip (#[ignore])"
```

---

### Task 8: README, CHANGELOG, and full CI gate

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces: the finished PR — docs updated and `just ci` green.

- [ ] **Step 1: Update the crate-table row**

In `README.md`, replace the `oath-adapter-ibkr` row (currently ending "…read-path wire layer; …to follow") with:

```markdown
| `oath-adapter-ibkr` | IBKR venue adapter — Client Portal API v1 (`cpapi`) read + order-write wire layer; `webapi` (beta OAuth) / `tws` (socket) surfaces to follow |
```

- [ ] **Step 2: Update the "coming soon" line**

In `README.md`, replace the closing-paragraph sentence about the venue adapter:

```markdown
The first venue adapter, `oath-adapter-ibkr`, covers the Client Portal API v1 read and order-write wire layers.
```

- [ ] **Step 3: Add the CHANGELOG entry**

In `CHANGELOG.md`, insert an `### Added` section immediately under `## [Unreleased]` (above the existing `### Changed`):

```markdown
### Added

- **`oath-adapter-ibkr` — CP API v1 order write-path wire layer.** Request-body serde
  DTOs (`OrderRequest`/`PlaceOrderRequest`/`ReplyConfirm` — the crate's first serialize
  direction), the order/reply union response (`OrderPlaceReply`, one all-optional struct),
  and cancel/status/live-orders response DTOs (`CancelResponse`, `OrderStatus`,
  `LiveOrders`/`LiveOrder`), plus `Endpoint` descriptors for place / reply-confirm /
  cancel / order-status / live-orders (`Method::Delete` added). No transport, auth, or
  OATH-domain translation (ADR-0022/0026 untouched). Fixtures are real, sanitized paper
  captures via an extended `just ibkr-capture` order dance; a gated `#[ignore]` live test
  drives a place → confirm → cancel round-trip.
```

- [ ] **Step 4: Run the full CI gate**

Run: `just ci`
Expected: PASS — `fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck` all green.

- [ ] **Step 5: Commit**

```bash
git add README.md CHANGELOG.md
git commit -m "docs(ibkr): README + CHANGELOG for cpapi order write path"
```

- [ ] **Step 6: Open the PR**

Push `feat/ibkr-cpapi-writepath` and open a PR with `Closes #135`, summarizing: CP API v1 order write-path wire layer (place/reply/cancel/status/live-orders) + harness order-dance capture + gated live round-trip; no domain translation.

---

## Self-Review

**Spec coverage:**
- §2 request DTOs (`OrderRequest`/`PlaceOrderRequest`/`ReplyConfirm`) → Task 2. ✅
- §2 response DTOs (`OrderPlaceReply` union; `CancelResponse`/`OrderStatus`/`LiveOrders`+`LiveOrder`) → Tasks 3, 4. ✅
- §3.3 endpoint descriptors (place/reply/cancel/status/live-orders; `Method::Delete`) → Task 1. ✅
- §3.1 module layout (new `order.rs`, re-exports, broadened `mod.rs` doc) → Tasks 2–4. ✅
- §3.2 harness order-dance capture + safety + sanitize + questions-branch fallback → Task 5; live run → Task 6. ✅
- §4 fixture decode + serialize tests + union coverage + gated live round-trip → Tasks 2, 3, 4, 7. ✅
- §5 workspace/lint conformance, README update, no new deps → Global Constraints + Task 8. ✅
- §6 DoD (order.rs + tests in `just ci`, harness extension, reconciled fixtures, gated live test, README/CHANGELOG, `just ci` green incl. `just doc`) → Tasks 6, 8. ✅
- §7.1 pure-wire boundary (no ADR-0022/0026) → out of scope by construction (no `Order`/`OrderId`, no safety logic). ✅
- §7.2 no `encode()` helper; focused field set; `Number` for money → Task 2. ✅
- §7.3 all-optional union, not untagged enum → Task 3. ✅
- §7.4 live-capture in-slice → Tasks 5, 6. ✅
- §7.5 modify deferred → out of scope by construction (no modify endpoint/DTO). ✅

**Placeholder scan:** every step ships concrete file content, an exact command, and expected output. Representative fixtures in Tasks 3–4 are explicitly reconciled to reality in Task 6 (a deliberate representative-then-real flow, not a placeholder). Field spellings/types marked "provisional" are the Task-6 reconcile targets.

**Type consistency:** `decode<T>` (read-path Task 3) is reused verbatim. `order.rs` re-exports in `mod.rs` accrue monotonically: Task 2 exports `{OrderRequest, PlaceOrderRequest, ReplyConfirm}`; Task 3 adds `OrderPlaceReply`; Task 4 adds `{CancelResponse, LiveOrder, LiveOrders, OrderStatus}` — final set matches every `pub` item in `order.rs`. `order_id` is `Option<String>` on `OrderPlaceReply` (place/reply) and `Option<i64>` on `OrderStatus`/`LiveOrder`/`CancelResponse` (status/live/cancel) — intentional and asserted. `LiveOrder` renames `orderId`/`orderType`/`totalSize` (camel-native endpoint); `OrderStatus` uses bare snake fields (snake-native endpoint). Request DTOs derive `Serialize`; response DTOs derive `Debug, Clone, Deserialize` (no `PartialEq`). `Method` gains `Delete` and keeps `PartialEq, Eq`. Serialize tests use round-trip-safe numbers (`1`, `185.5`) per the `arbitrary_precision`-off constraint.
