//! Order write-path wire layer: request bodies for placing and confirming orders,
//! plus the order-lifecycle response DTOs (added in later tasks).
//!
//! Faithfully mirrors Client Portal API v1 JSON — no transport, no auth, no
//! OATH-domain translation, no order-safety semantics. `side`, `orderType`, and
//! `tif` are kept as `String` (the wire's own tokens); the mapping onto OATH's
//! domain types is the deferred translation layer's job.

use serde::Serialize;

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
