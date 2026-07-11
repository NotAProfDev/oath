//! Tests for the cpapi order write-path DTOs (request serialize + response decode).
use oath_adapter_ibkr::cpapi::{OrderRequest, PlaceOrderRequest, ReplyConfirm};

#[test]
fn order_request_serializes_with_renames_and_omits_absent_optionals() {
    // Round-trip-safe numbers only: serde_json has no arbitrary_precision, so a
    // trailing-zero decimal like 185.50 would serialize back as 185.5.
    let req = OrderRequest {
        conid: 265_598,
        side: "BUY".to_owned(),
        order_type: "LMT".to_owned(),
        quantity: serde_json::Number::from(1_u64),
        tif: "DAY".to_owned(),
        price: Some("185.5".parse().expect("valid number")),
        aux_price: None,
        coid: Some("oath-0001".to_owned()),
        outside_rth: Some(false),
    };
    let json = serde_json::to_string(&req).expect("serializes");
    assert_eq!(
        json,
        r#"{"conid":265598,"side":"BUY","orderType":"LMT","quantity":1,"tif":"DAY","price":185.5,"cOID":"oath-0001","outsideRTH":false}"#
    );
}

#[test]
fn place_order_request_wraps_orders_array() {
    let req = PlaceOrderRequest {
        orders: vec![OrderRequest {
            conid: 265_598,
            side: "SELL".to_owned(),
            order_type: "MKT".to_owned(),
            quantity: serde_json::Number::from(2_u64),
            tif: "DAY".to_owned(),
            price: None,
            aux_price: None,
            coid: None,
            outside_rth: None,
        }],
    };
    let json = serde_json::to_string(&req).expect("serializes");
    assert_eq!(
        json,
        r#"{"orders":[{"conid":265598,"side":"SELL","orderType":"MKT","quantity":2,"tif":"DAY"}]}"#
    );
}

#[test]
fn reply_confirm_serializes() {
    let json = serde_json::to_string(&ReplyConfirm { confirmed: true }).expect("serializes");
    assert_eq!(json, r#"{"confirmed":true}"#);
}
