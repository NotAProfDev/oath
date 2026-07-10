//! Fixture tests for the portfolio DTOs.
use oath_adapter_ibkr::cpapi::{IServerAccounts, PortfolioAccount, decode};

#[test]
fn iserver_accounts_deserializes() {
    let accts: IServerAccounts = decode(include_bytes!("fixtures/cpapi/iserver_accounts.json"))
        .expect("iserver accounts decodes");
    assert_eq!(accts.accounts, vec!["DU0000000".to_owned()]);
    assert_eq!(accts.selected_account.as_deref(), Some("DU0000000"));
}

#[test]
fn portfolio_accounts_deserializes() {
    let accts: Vec<PortfolioAccount> =
        decode(include_bytes!("fixtures/cpapi/portfolio_accounts.json"))
            .expect("portfolio accounts decodes");
    assert_eq!(accts.len(), 1);
    let first = accts.first().expect("one account");
    assert_eq!(first.id, "DU0000000");
    assert_eq!(first.account_type.as_deref(), Some("DEMO"));
}

#[test]
fn positions_deserialize_conid_as_int_and_money_as_number() {
    use oath_adapter_ibkr::cpapi::Position;
    let positions: Vec<Position> =
        decode(include_bytes!("fixtures/cpapi/positions.json")).expect("positions decode");
    let p = positions.first().expect("one position");
    assert_eq!(p.conid, 265_598);
    // Money stays a serde_json::Number — faithful to the wire, no premature f64.
    assert_eq!(
        p.mkt_price.as_ref().map(ToString::to_string).as_deref(),
        Some("314.341156")
    );
}
