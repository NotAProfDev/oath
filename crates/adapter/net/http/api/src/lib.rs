//! `oath-adapter-net-http-api` — the HTTP transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`) and adds the request/reply [`Service`] connection shape. The HTTP
//! data plane (`HttpError`, `HttpClient`, `ResponseBody`, the layers) lands in
//! later slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
#![forbid(unsafe_code)]

pub mod service;

pub use service::Service;
