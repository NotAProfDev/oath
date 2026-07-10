//! Endpoint descriptors for the Client Portal API v1 read path.
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
}

/// A Client Portal API v1 endpoint: an HTTP [`Method`] and a path relative to the
/// `/v1/api` base (for example `/portfolio/accounts`).
///
/// This descriptor models the HTTP method and the path — including any query
/// string — only. Request **bodies** (for example the `secdef_search` search
/// payload `{symbol, secType}`) are supplied by the future request/transport
/// binding and are not modeled in this read-path slice.
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

    /// `GET /iserver/secdef/info` — contract details (call after `secdef_search`).
    /// `conid` and `sec_type` are required query params.
    #[must_use]
    pub fn secdef_info(conid: i64, sec_type: &str) -> Self {
        Self {
            method: Method::Get,
            path: format!("/iserver/secdef/info?conid={conid}&secType={sec_type}"),
        }
    }
}
