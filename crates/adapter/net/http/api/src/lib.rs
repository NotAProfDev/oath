//! `oath-adapter-net-http-api` — the HTTP transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`). Defines the HTTP transport contract:
//!
//! - [`service`] — the `Service` request/reply connection shape
//! - [`error`] — `HttpError` and its `HasErrorKind` impl
//! - [`client`] — the `HttpClient` dependency-inversion seam
//! - [`body`] — `ResponseBody` and `BufferMode`
//! - [`auth`] — the `AuthSource` seam, `NoAuth`, and the `Auth`/`SetHeaders` layers
//!
//! The resilience layers, `stack`/`build` assembly, and backends land in later
//! slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
#![forbid(unsafe_code)]

pub mod auth;
pub mod body;
pub mod client;
pub mod error;
pub mod service;

pub use auth::{Auth, AuthSource, NoAuth};
pub use body::{BufferMode, ResponseBody};
pub use client::HttpClient;
pub use error::{BoxError, HttpError};
pub use service::Service;
