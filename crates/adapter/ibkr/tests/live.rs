//! Live integration test against a running, authenticated Client Portal Gateway.
//!
//! `#[ignore]` keeps it out of `just ci` — `just test` runs `--all-features`, so a
//! cargo feature would NOT exclude it, but nextest/cargo test skip ignored tests.
//! Run it explicitly (gateway up + logged in at https://localhost:5000):
//!   cargo test -p oath-adapter-ibkr --test live -- --ignored
//!   # or: cargo nextest run -p oath-adapter-ibkr --run-ignored
//!
//! An `IBKR_GATEWAY` override must include the `/v1/api` path segment (the default
//! is `https://localhost:5000/v1/api`).
use std::process::Command;

use oath_adapter_ibkr::cpapi::{
    AuthStatus, CancelResponse, Endpoint, OrderPlaceReply, OrderRequest, PlaceOrderRequest,
    ReplyConfirm, decode,
};

#[test]
#[ignore = "requires a live, authenticated Client Portal Gateway on https://localhost:5000"]
fn live_auth_status_deserializes() {
    let base = std::env::var("IBKR_GATEWAY")
        .unwrap_or_else(|_| "https://localhost:5000/v1/api".to_owned());
    // -f: fail on HTTP 4xx/5xx; -k: skip TLS verify (the gateway ships a self-signed cert);
    // --max-time: fail fast instead of hanging on a stalled/partially-reachable gateway.
    let output = Command::new("curl")
        .args([
            "-fksS",
            "--max-time",
            "30",
            "-X",
            "GET",
            &format!("{base}/iserver/auth/status"),
        ])
        .output()
        .expect("curl should run");
    assert!(output.status.success(), "curl failed: {output:?}");
    // Decoding is the assertion; `authenticated` depends on live login state.
    let _status: AuthStatus =
        decode(&output.stdout).expect("live auth/status should decode into AuthStatus");
}

/// Best-effort cancel-on-drop, so a panic between placing an order and the
/// explicit cancel cannot strand a resting paper order. Arm it (`order_id`) once
/// the order id is known; disarm it after a successful explicit cancel.
struct CancelGuard<'a> {
    base: &'a str,
    account: &'a str,
    order_id: Option<String>,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if let Some(order_id) = &self.order_id {
            let _ = Command::new("curl")
                .args([
                    "-fksS",
                    "--max-time",
                    "30",
                    "-X",
                    "DELETE",
                    &format!(
                        "{}/iserver/account/{}/order/{order_id}",
                        self.base, self.account
                    ),
                ])
                .output();
        }
    }
}

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
    let conid: i64 = std::env::var("IBKR_CONID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(265_598);

    let curl = |args: &[&str]| -> Vec<u8> {
        let output = Command::new("curl")
            .args(["-fksS", "--max-time", "30"])
            .args(args)
            .output()
            .expect("curl should run");
        assert!(output.status.success(), "curl failed: {output:?}");
        output.stdout
    };

    // Place a far-below-market resting LIMIT BUY (will not fill), built via the request DTOs.
    let place_body = serde_json::to_string(&PlaceOrderRequest {
        orders: vec![OrderRequest {
            conid,
            side: "BUY".to_owned(),
            order_type: "LMT".to_owned(),
            quantity: serde_json::Number::from(1_u64),
            tif: "DAY".to_owned(),
            price: Some(serde_json::Number::from(1_u64)),
            aux_price: None,
            coid: None,
            outside_rth: Some(false),
        }],
    })
    .expect("place body serializes");
    let place_url = format!("{base}{}", Endpoint::place_orders(&account).path);
    let placed = curl(&[
        "-X",
        "POST",
        &place_url,
        "-H",
        "Content-Type: application/json",
        "-d",
        &place_body,
    ]);
    let mut replies: Vec<OrderPlaceReply> = decode(&placed).expect("place decodes");

    // Arm the cancel guard the moment an order id is known — a plain place may confirm
    // directly, otherwise each confirmed reply reveals it — so a panic mid-flow cannot
    // strand a resting order.
    let mut guard = CancelGuard {
        base: &base,
        account: &account,
        order_id: replies.first().and_then(|r| r.order_id.clone()),
    };

    // Confirm the reply chain until an order_id appears (bounded).
    let reply_body =
        serde_json::to_string(&ReplyConfirm { confirmed: true }).expect("reply body serializes");
    for _ in 0..5 {
        let Some(reply_id) = replies.first().and_then(|r| r.id.clone()) else {
            break;
        };
        let reply_url = format!("{base}{}", Endpoint::reply(&reply_id).path);
        let confirmed = curl(&[
            "-X",
            "POST",
            &reply_url,
            "-H",
            "Content-Type: application/json",
            "-d",
            &reply_body,
        ]);
        replies = decode(&confirmed).expect("reply decodes");
        guard.order_id = replies.first().and_then(|r| r.order_id.clone());
    }
    let order_id = replies
        .first()
        .and_then(|r| r.order_id.clone())
        .expect("a confirmed order_id");
    guard.order_id = Some(order_id.clone());

    // Explicit cancel — decode the ack, then disarm the guard (order already cancelled).
    let cancel_url = format!("{base}{}", Endpoint::cancel_order(&account, &order_id).path);
    let cancelled = curl(&["-X", "DELETE", &cancel_url]);
    let _resp: CancelResponse = decode(&cancelled).expect("cancel decodes");
    guard.order_id = None;
}
