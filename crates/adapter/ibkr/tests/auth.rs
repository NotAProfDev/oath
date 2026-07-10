//! Fixture tests for the auth/session DTOs.
use oath_adapter_ibkr::cpapi::{AuthStatus, TickleResponse, decode};

#[test]
fn auth_status_deserializes() {
    let status: AuthStatus =
        decode(include_bytes!("fixtures/cpapi/auth_status.json")).expect("auth_status decodes");
    assert!(status.authenticated);
    assert!(status.connected);
    assert!(!status.competing);
}

#[test]
fn tickle_relays_iserver_auth_status() {
    let tickle: TickleResponse =
        decode(include_bytes!("fixtures/cpapi/tickle.json")).expect("tickle decodes");
    assert!(!tickle.session.is_empty());
    let iserver = tickle.iserver.expect("tickle relays the iserver block");
    assert!(iserver.auth_status.authenticated);
}

#[test]
fn tickle_binds_ibkrs_misspelled_collission_key() {
    // IBKR sends the collision flag under the misspelled key `collission`; ensure
    // our rename binds it (a correctly-spelled key would silently default to false).
    let tickle: TickleResponse = decode(br#"{"session":"s","collission":true}"#).expect("decodes");
    assert!(tickle.collision);
}
