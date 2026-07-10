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
