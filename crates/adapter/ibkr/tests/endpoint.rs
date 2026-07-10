//! Endpoint path-rendering tests — one assertion group per constructor, covering
//! all seven Client Portal API v1 read-path endpoints.
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
fn secdef_info_path_interpolates_conid_and_sec_type() {
    let ep = Endpoint::secdef_info(265_598, "STK");
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/iserver/secdef/info?conid=265598&secType=STK");
}
