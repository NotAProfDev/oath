//! Tests for the cpapi order write-path DTOs (request serialize + response decode).
use oath_adapter_ibkr::cpapi::{
    CancelResponse, LiveOrders, OrderPlaceReply, OrderRequest, OrderStatus, PlaceOrderRequest,
    ReplyConfirm,
};

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

#[test]
fn order_place_questions_decode_as_question_shape() {
    use oath_adapter_ibkr::cpapi::decode;
    let replies: Vec<OrderPlaceReply> =
        decode(include_bytes!("fixtures/cpapi/order_place_questions.json"))
            .expect("questions decode");
    let q = replies.first().expect("one reply");
    assert_eq!(
        q.id.as_deref(),
        Some("a1b2c3d4-0000-0000-0000-000000000000")
    );
    assert_eq!(q.message.as_ref().map(Vec::len), Some(1));
    assert_eq!(q.is_suppressed, Some(false));
    // Confirmation fields are absent on a question.
    assert!(q.order_id.is_none());
    assert!(q.order_status.is_none());
    assert!(q.encrypt_message.is_none());
}

#[test]
fn order_reply_confirmed_decodes_as_confirmation_shape() {
    use oath_adapter_ibkr::cpapi::decode;
    let replies: Vec<OrderPlaceReply> =
        decode(include_bytes!("fixtures/cpapi/order_reply_confirmed.json"))
            .expect("confirmation decode");
    let c = replies.first().expect("one reply");
    assert_eq!(c.order_id.as_deref(), Some("1234567890"));
    assert_eq!(c.order_status.as_deref(), Some("PreSubmitted"));
    // Question fields are absent on a confirmation.
    assert!(c.id.is_none());
    assert!(c.message.is_none());
    assert!(c.is_suppressed.is_none());
}

#[test]
fn cancel_response_decodes() {
    use oath_adapter_ibkr::cpapi::decode;
    let resp: CancelResponse =
        decode(include_bytes!("fixtures/cpapi/order_cancel.json")).expect("cancel decodes");
    assert_eq!(resp.order_id, Some(1_234_567_890));
    assert_eq!(resp.msg.as_deref(), Some("Request was submitted"));
}

#[test]
fn order_status_decodes_snake_fields() {
    use oath_adapter_ibkr::cpapi::decode;
    let status: OrderStatus =
        decode(include_bytes!("fixtures/cpapi/order_status.json")).expect("status decodes");
    assert_eq!(status.order_id, Some(1_234_567_890));
    assert_eq!(status.order_status.as_deref(), Some("PreSubmitted"));
    // Sizes arrive as strings on this endpoint — kept faithfully as String.
    assert_eq!(status.total_size.as_deref(), Some("1"));
}

#[test]
fn live_orders_decode_camel_fields() {
    use oath_adapter_ibkr::cpapi::decode;
    let live: LiveOrders =
        decode(include_bytes!("fixtures/cpapi/live_orders.json")).expect("live orders decode");
    assert_eq!(live.snapshot, Some(false));
    let o = live.orders.first().expect("one live order");
    assert_eq!(o.order_id, Some(1_234_567_890)); // renamed from "orderId"
    assert_eq!(o.ticker.as_deref(), Some("AAPL"));
    assert_eq!(o.order_type.as_deref(), Some("LIMIT")); // renamed from "orderType"
}
