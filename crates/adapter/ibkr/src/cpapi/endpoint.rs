//! Endpoint descriptors for the Client Portal API v1 — the read path and the
//! order write path.
//!
//! An [`Endpoint`] is a pure value — an HTTP [`Method`] plus a path *relative to the
//! gateway base URL* (`https://localhost:5000/v1/api`). This layer carries no
//! transport; a future HTTP binding turns an `Endpoint` into a request.

/// HTTP method for a Client Portal API v1 [`Endpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// HTTP `GET`.
    Get,
    /// HTTP `POST`.
    Post,
    /// HTTP `DELETE`.
    Delete,
}

/// A Client Portal API v1 endpoint: an HTTP [`Method`] and a path relative to the
/// `/v1/api` base (for example `/portfolio/accounts`).
///
/// This descriptor models the HTTP method and the path — including any query
/// string — only. Request **bodies** (for example the `secdef_search` search
/// payload `{symbol, secType}`, or the order-submission and reply-confirm bodies
/// of the write path) are supplied by the future request/transport binding and
/// are not modeled here.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The HTTP method.
    pub method: Method,
    /// The path, relative to the `/v1/api` base.
    pub path: String,
}

impl Endpoint {
    /// `GET /iserver/auth/status` — current authentication / brokerage-session status.
    #[must_use]
    pub fn auth_status() -> Self {
        Self {
            method: Method::Get,
            path: "/iserver/auth/status".to_owned(),
        }
    }

    /// `POST /tickle` — session keepalive; also relays the `iserver` auth status.
    #[must_use]
    pub fn tickle() -> Self {
        Self {
            method: Method::Post,
            path: "/tickle".to_owned(),
        }
    }

    /// `GET /iserver/accounts` — accounts the user can trade.
    #[must_use]
    pub fn iserver_accounts() -> Self {
        Self {
            method: Method::Get,
            path: "/iserver/accounts".to_owned(),
        }
    }

    /// `GET /portfolio/accounts` — accounts for portfolio/position queries; must be
    /// called before other `/portfolio` endpoints.
    #[must_use]
    pub fn portfolio_accounts() -> Self {
        Self {
            method: Method::Get,
            path: "/portfolio/accounts".to_owned(),
        }
    }

    /// `GET /portfolio/{account_id}/positions/{page}` — one page of positions.
    #[must_use]
    pub fn positions(account_id: &str, page: u32) -> Self {
        Self {
            method: Method::Get,
            path: format!("/portfolio/{account_id}/positions/{page}"),
        }
    }

    /// `POST /iserver/secdef/search` — contract search by symbol / company name.
    #[must_use]
    pub fn secdef_search() -> Self {
        Self {
            method: Method::Post,
            path: "/iserver/secdef/search".to_owned(),
        }
    }

    /// `GET /iserver/secdef/info?conid={conid}` — contract details for a stock
    /// (call after `secdef_search`). Only `conid` is sent: a live paper gateway
    /// rejects `secType=STK` here with `400 "month required"`, because the `secType`
    /// path is for options/futures (which additionally need `month`/`strike`/`right`
    /// — a future slice).
    #[must_use]
    pub fn secdef_info(conid: i64) -> Self {
        Self {
            method: Method::Get,
            path: format!("/iserver/secdef/info?conid={conid}"),
        }
    }

    /// `POST /iserver/account/{account_id}/orders` — submit one or more orders.
    /// The body (a `PlaceOrderRequest`) is supplied by the transport, not this descriptor.
    #[must_use]
    pub fn place_orders(account_id: &str) -> Self {
        Self {
            method: Method::Post,
            path: format!("/iserver/account/{account_id}/orders"),
        }
    }

    /// `POST /iserver/reply/{reply_id}` — confirm a suppressible order warning
    /// (body `{"confirmed":true}`, a `ReplyConfirm`).
    #[must_use]
    pub fn reply(reply_id: &str) -> Self {
        Self {
            method: Method::Post,
            path: format!("/iserver/reply/{reply_id}"),
        }
    }

    /// `DELETE /iserver/account/{account_id}/order/{order_id}` — cancel a live order.
    #[must_use]
    pub fn cancel_order(account_id: &str, order_id: &str) -> Self {
        Self {
            method: Method::Delete,
            path: format!("/iserver/account/{account_id}/order/{order_id}"),
        }
    }

    /// `GET /iserver/account/order/status/{order_id}` — status of a single order.
    #[must_use]
    pub fn order_status(order_id: &str) -> Self {
        Self {
            method: Method::Get,
            path: format!("/iserver/account/order/status/{order_id}"),
        }
    }

    /// `GET /iserver/account/orders` — the account's live orders.
    #[must_use]
    pub fn live_orders() -> Self {
        Self {
            method: Method::Get,
            path: "/iserver/account/orders".to_owned(),
        }
    }
}
