//! Client Portal API v1 (`cpapi`) wire layer: endpoint descriptors and serde DTOs for
//! the read path and the order write path.
//!
//! Endpoint descriptors and serde DTOs that mirror IBKR's JSON responses. No auth,
//! no transport, no OATH-domain translation.
//!
//! The DTOs faithfully mirror the *modeled* fields; unmodeled fields (for
//! example `assetClass`, `isPaper`, `hmds`) are silently ignored, not echoed
//! back — this is a faithful subset, not a byte-for-byte round trip.

pub mod auth;
pub mod endpoint;
pub mod error;
pub mod order;
pub mod portfolio;
pub mod secdef;

pub use auth::{AuthStatus, ServerInfo, TickleIServer, TickleResponse};
pub use endpoint::{Endpoint, Method};
pub use error::{CpapiError, WireError, decode};
pub use order::{OrderRequest, PlaceOrderRequest, ReplyConfirm};
pub use portfolio::{IServerAccounts, PortfolioAccount, Position};
pub use secdef::{SecdefInfo, SecdefSearchEntry, SecdefSection};
