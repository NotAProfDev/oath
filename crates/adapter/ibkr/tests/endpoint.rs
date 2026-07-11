//! Endpoint path-rendering tests — one assertion group per constructor, covering
//! the Client Portal API v1 read-path and order write-path endpoints.
use oath_adapter_ibkr::cpapi::{Endpoint, Method};

#[test]
fn auth_status_is_a_get_to_iserver_auth_status() {
    let ep = Endpoint::auth_status();
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/auth/status");
}

#[test]
fn tickle_is_a_post_to_slash_tickle() {
    let ep = Endpoint::tickle();
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/tickle");
}

#[test]
fn iserver_accounts_is_a_get_to_iserver_accounts() {
    let ep = Endpoint::iserver_accounts();
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/accounts");
}

#[test]
fn portfolio_accounts_is_a_get_to_portfolio_accounts() {
    let ep = Endpoint::portfolio_accounts();
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/portfolio/accounts");
}

#[test]
fn positions_path_interpolates_account_and_page() {
    let ep = Endpoint::positions("U1234567", 0);
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/portfolio/U1234567/positions/0");
}

#[test]
fn secdef_search_is_a_post_to_iserver_secdef_search() {
    let ep = Endpoint::secdef_search();
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/iserver/secdef/search");
}

#[test]
fn secdef_info_path_interpolates_conid_only() {
    // A live paper gateway rejects `secType=STK` on this endpoint with
    // 400 "month required"; a stock lookup passes `conid` alone.
    let ep = Endpoint::secdef_info(265_598);
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/secdef/info?conid=265598");
}

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
