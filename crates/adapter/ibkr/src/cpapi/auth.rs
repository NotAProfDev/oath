//! Session/auth read endpoints: `iserver/auth/status` and `tickle`.

use serde::Deserialize;

/// Server identity block embedded in an [`AuthStatus`].
#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    /// Server name.
    #[serde(rename = "serverName")]
    pub server_name: Option<String>,
    /// Server version string.
    #[serde(rename = "serverVersion")]
    pub server_version: Option<String>,
}

/// Response of `GET|POST /iserver/auth/status` — the brokerage-session state.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthStatus {
    /// `true` once initial authentication passes.
    pub authenticated: bool,
    /// `true` when another session is competing for the same account.
    pub competing: bool,
    /// `true` when connected to the brokerage backend.
    pub connected: bool,
    /// Optional status message.
    #[serde(default)]
    pub message: String,
    /// Machine access code, when present.
    #[serde(rename = "MAC")]
    pub mac: Option<String>,
    /// Failure reason; empty when healthy.
    #[serde(default)]
    pub fail: String,
    /// Server identity, when present.
    #[serde(rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

/// The `iserver` block of a [`TickleResponse`], wrapping the auth status.
#[derive(Debug, Clone, Deserialize)]
pub struct TickleIServer {
    /// The embedded auth status.
    #[serde(rename = "authStatus")]
    pub auth_status: AuthStatus,
}

/// Response of `POST /tickle` — session keepalive; also relays the auth status.
#[derive(Debug, Clone, Deserialize)]
pub struct TickleResponse {
    /// Opaque session token.
    pub session: String,
    /// SSO expiry, in seconds, when present.
    #[serde(rename = "ssoExpires")]
    pub sso_expires: Option<i64>,
    /// `true` when a session collision occurred.
    #[serde(default)]
    pub collision: bool,
    /// Numeric user id, when present.
    #[serde(rename = "userId")]
    pub user_id: Option<i64>,
    /// The relayed `iserver` auth block, when present.
    pub iserver: Option<TickleIServer>,
}
