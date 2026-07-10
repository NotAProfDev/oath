//! Tests for the CP API v1 error envelope and the `decode` entry point.
use oath_adapter_ibkr::cpapi::{CpapiError, WireError, decode};

#[test]
fn error_envelope_decodes() {
    let bytes = br#"{"error":"no bridge","statusCode":401}"#;
    let err: CpapiError = decode(bytes).expect("error envelope should decode");
    assert_eq!(err.error, "no bridge");
    assert_eq!(err.status_code, Some(401));
}

#[test]
fn error_envelope_without_status_code_decodes() {
    let bytes = br#"{"error":"Please query /accounts first"}"#;
    let err: CpapiError = decode(bytes).expect("bare error should decode");
    assert_eq!(err.status_code, None);
}

#[test]
fn malformed_json_is_a_wire_error() {
    let bytes = b"not json";
    let result: Result<CpapiError, WireError> = decode(bytes);
    assert!(matches!(result, Err(WireError::Json(_))));
}
