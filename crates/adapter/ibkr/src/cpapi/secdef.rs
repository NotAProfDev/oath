//! Contract search/info read endpoints: `iserver/secdef/search` and
//! `iserver/secdef/info`.

use serde::Deserialize;

/// A tradable section within a `SecdefSearchEntry` (for example `STK`, `OPT`).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefSection {
    /// Security type, for example `"STK"`, `"OPT"`.
    #[serde(rename = "secType")]
    pub sec_type: String,
    /// Available expiry months (`OPT`/`FUT`), when present.
    pub months: Option<String>,
}

/// One element of `POST /iserver/secdef/search`.
///
/// `conid` is a **string** on this endpoint — the same logical id is an integer on
/// the positions and `secdef/info` endpoints. Modelling each as the wire actually
/// sends it (not a forced shared type) is the faithful-mirror rule (spec §7.4).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefSearchEntry {
    /// IBKR contract id (a string on this endpoint).
    pub conid: String,
    /// Company name, when present.
    #[serde(rename = "companyName")]
    pub company_name: Option<String>,
    /// Symbol, when present.
    pub symbol: Option<String>,
    /// Free-text description (often the exchange), when present.
    pub description: Option<String>,
    /// Tradable sections by security type.
    #[serde(default)]
    pub sections: Vec<SecdefSection>,
}

/// One element of `GET /iserver/secdef/info`.
///
/// `conid` is an **integer** here (contrast `SecdefSearchEntry`).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefInfo {
    /// IBKR contract id (an integer on this endpoint).
    pub conid: i64,
    /// Symbol, when present.
    pub symbol: Option<String>,
    /// Security type, when present.
    #[serde(rename = "secType")]
    pub sec_type: Option<String>,
    /// Primary exchange, when present.
    pub exchange: Option<String>,
    /// Contract currency, when present.
    pub currency: Option<String>,
    /// Company name, when present.
    #[serde(rename = "companyName")]
    pub company_name: Option<String>,
}
