//! Portfolio read endpoints: `iserver/accounts`, `portfolio/accounts`, and
//! `portfolio/{account}/positions/{page}`.

use serde::Deserialize;

/// Response of `GET /iserver/accounts` — accounts the user can trade.
#[derive(Debug, Clone, Deserialize)]
pub struct IServerAccounts {
    /// Tradable account ids.
    pub accounts: Vec<String>,
    /// The currently selected account, when present.
    #[serde(rename = "selectedAccount")]
    pub selected_account: Option<String>,
}

/// One element of `GET /portfolio/accounts` — an account for portfolio queries.
#[derive(Debug, Clone, Deserialize)]
pub struct PortfolioAccount {
    /// Account id (for example `"DU0000000"`).
    pub id: String,
    /// Account id (a duplicate field IBKR also returns), when present.
    #[serde(rename = "accountId")]
    pub account_id: Option<String>,
    /// Base currency, when present.
    pub currency: Option<String>,
    /// Account type — IBKR's `type` field (`"DEMO"` for paper), when present.
    #[serde(rename = "type")]
    pub account_type: Option<String>,
}

/// One element of `GET /portfolio/{account}/positions/{page}`.
///
/// `conid` is an **integer** on this endpoint (contrast `secdef/search`, where the
/// same logical id arrives as a *string* — see `SecdefSearchEntry`). Monetary and
/// quantity fields are kept as `serde_json::Number`: faithful to the wire, precision
/// preserved, no premature `f64`. Conversion to fixed-point (ADR-0023) is the future
/// translation layer's job, not the wire's.
#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    /// Account id owning the position, when present.
    #[serde(rename = "acctId")]
    pub acct_id: Option<String>,
    /// IBKR contract id (integer on this endpoint).
    pub conid: i64,
    /// Signed position size, when present.
    pub position: Option<serde_json::Number>,
    /// Market price, when present.
    #[serde(rename = "mktPrice")]
    pub mkt_price: Option<serde_json::Number>,
    /// Market value, when present.
    #[serde(rename = "mktValue")]
    pub mkt_value: Option<serde_json::Number>,
    /// Position currency, when present.
    pub currency: Option<String>,
    /// Contract description, when present.
    #[serde(rename = "contractDesc")]
    pub contract_desc: Option<String>,
}
