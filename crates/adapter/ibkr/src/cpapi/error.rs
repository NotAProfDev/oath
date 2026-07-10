//! The Client Portal API v1 error envelope, this crate's decode error type, and the
//! [`decode`] entry point for turning a response body into a typed value.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// The JSON error body IBKR returns for a failed Client Portal API v1 request,
/// for example `{"error":"no bridge","statusCode":401}`.
#[derive(Debug, Clone, Deserialize)]
pub struct CpapiError {
    /// Human-readable error message.
    pub error: String,
    /// HTTP-style status code, when present.
    #[serde(rename = "statusCode")]
    pub status_code: Option<i64>,
}

/// An error decoding a Client Portal API v1 response body.
#[derive(Debug, Error)]
pub enum WireError {
    /// The body was not valid JSON for the target type.
    #[error("malformed Client Portal API JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Deserialize a Client Portal API v1 response body into `T`.
///
/// The wire layer carries no transport, so the caller (a future HTTP binding)
/// decides — from the HTTP status — whether to `decode::<T>` a success body or
/// `decode::<CpapiError>` an error body.
///
/// # Errors
///
/// Returns [`WireError::Json`] if `bytes` is not valid JSON for `T`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    Ok(serde_json::from_slice(bytes)?)
}
