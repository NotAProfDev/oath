//! Endpoint path-rendering tests.
use oath_adapter_ibkr::cpapi::{Endpoint, Method};

#[test]
fn positions_path_interpolates_account_and_page() {
    let ep = Endpoint::positions("U1234567", 0);
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/portfolio/U1234567/positions/0");
}

#[test]
fn tickle_is_a_post_to_slash_tickle() {
    let ep = Endpoint::tickle();
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/tickle");
}

#[test]
fn secdef_search_is_a_post() {
    assert_eq!(Endpoint::secdef_search().method, Method::Post);
}
