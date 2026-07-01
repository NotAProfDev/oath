//! `oath-adapter-net-http-api` — the HTTP transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`) and adds the request/reply [`Service`] connection shape. The HTTP
//! data plane (`HttpError`, `HttpClient`, `ResponseBody`, the layers) lands in
//! later slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
//!
//! - [`error`] — `HttpError` and `HasErrorKind` impl
//! - [`client`] — `HttpClient` dependency-inversion seam
#![forbid(unsafe_code)]

pub mod body;
pub mod client;
pub mod error;
pub mod service;

pub use body::{BufferMode, ResponseBody};
pub use client::HttpClient;
pub use error::{BoxError, HttpError};
pub use service::Service;
