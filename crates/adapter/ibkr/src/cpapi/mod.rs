//! Client Portal API v1 (`cpapi`) read-path wire layer: endpoint descriptors and
//! serde DTOs that mirror IBKR's JSON responses losslessly. No auth, no transport,
//! no OATH-domain translation.

pub mod auth;
pub mod endpoint;
pub mod error;
pub mod portfolio;

pub use auth::{AuthStatus, ServerInfo, TickleIServer, TickleResponse};
pub use endpoint::{Endpoint, Method};
pub use error::{CpapiError, WireError, decode};
pub use portfolio::{IServerAccounts, PortfolioAccount};
