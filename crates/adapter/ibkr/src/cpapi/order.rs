//! Order write-path wire layer: request bodies for placing and confirming orders,
//! plus the order-lifecycle response DTOs (place/reply union, cancel, status, live orders).
//!
//! Faithfully mirrors Client Portal API v1 JSON — no transport, no auth, no
//! OATH-domain translation, no order-safety semantics. `side`, `orderType`, and
//! `tif` are kept as `String` (the wire's own tokens); the mapping onto OATH's
//! domain types is the deferred translation layer's job.

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
/// This endpoint is **`snake_case`**-native — no serde renames. Sizes and the limit
/// price arrive as strings (kept faithfully as `String`).
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
    /// Limit price, sent as a string (the wire field is `limit_price`), when present.
    pub limit_price: Option<String>,
    /// Time in force, when present.
    pub tif: Option<String>,
    /// Account id, when present.
    pub account: Option<String>,
}

/// Response of `GET /iserver/account/orders` — the account's live orders.
///
/// This endpoint is **`camelCase`**-native, so each `LiveOrder` renames `orderId` /
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
    /// Order id (integer; `camelCase` `orderId` on this endpoint).
    #[serde(rename = "orderId")]
    pub order_id: Option<i64>,
    /// Ticker symbol, when present.
    pub ticker: Option<String>,
    /// Side token, when present.
    pub side: Option<String>,
    /// Order status, when present.
    pub status: Option<String>,
    /// Order type (`camelCase` `orderType` on this endpoint), when present.
    #[serde(rename = "orderType")]
    pub order_type: Option<String>,
    /// Total order size (`camelCase` `totalSize`), when present.
    #[serde(rename = "totalSize")]
    pub total_size: Option<serde_json::Number>,
    /// Limit price, sent as a string on this endpoint, when present.
    pub price: Option<String>,
}
