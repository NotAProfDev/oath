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

use oath_adapter_ibkr::cpapi::{AuthStatus, decode};

#[test]
#[ignore = "requires a live, authenticated Client Portal Gateway on https://localhost:5000"]
fn live_auth_status_deserializes() {
    let base = std::env::var("IBKR_GATEWAY")
        .unwrap_or_else(|_| "https://localhost:5000/v1/api".to_owned());
    let output = Command::new("curl")
        .args(["-fksS", "-X", "GET", &format!("{base}/iserver/auth/status")])
        .output()
        .expect("curl should run");
    assert!(output.status.success(), "curl failed: {output:?}");
    // Decoding is the assertion; `authenticated` depends on live login state.
    let _status: AuthStatus =
        decode(&output.stdout).expect("live auth/status should decode into AuthStatus");
}
