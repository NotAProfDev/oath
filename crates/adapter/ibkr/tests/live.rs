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

use oath_adapter_ibkr::cpapi::{AuthStatus, CancelResponse, OrderPlaceReply, decode};

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
        "-X",
        "POST",
        &format!("{base}/iserver/account/{account}/orders"),
        "-H",
        "Content-Type: application/json",
        "-d",
        &body,
    ]);
    let mut replies: Vec<OrderPlaceReply> = decode(&placed).expect("place decodes");

    // Confirm the reply chain until an order_id appears (bounded).
    for _ in 0..5 {
        let Some(reply_id) = replies.first().and_then(|r| r.id.clone()) else {
            break;
        };
        let confirmed = curl(&[
            "-X",
            "POST",
            &format!("{base}/iserver/reply/{reply_id}"),
            "-H",
            "Content-Type: application/json",
            "-d",
            r#"{"confirmed":true}"#,
        ]);
        replies = decode(&confirmed).expect("reply decodes");
    }
    let order_id = replies
        .first()
        .and_then(|r| r.order_id.clone())
        .expect("a confirmed order_id");

    // Cancel — always, so the round-trip leaves no resting order.
    let cancelled = curl(&[
        "-X",
        "DELETE",
        &format!("{base}/iserver/account/{account}/order/{order_id}"),
    ]);
    let _resp: CancelResponse = decode(&cancelled).expect("cancel decodes");
}
