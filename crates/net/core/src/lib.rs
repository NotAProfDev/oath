//! `oath-net-core` — composition primitives and capability trait contracts.
//!
//! This crate is **zero I/O, zero runtime**. It defines the shared
//! abstractions that every layer in the network stack depends on:
//!
//! - [`service`] — `Service`, `Layer`, `ServiceBuilder`, `Identity`, `Stack`
//! - [`error_kind`] — `ErrorKind`, `HasErrorKind`
//!
//! No `tokio`, `hyper`, `reqwest`, `serde`, or `thiserror` may appear in this
//! crate's dependency graph.
#![forbid(unsafe_code)]

pub mod error_kind;
pub mod service;

pub use error_kind::{ErrorKind, HasErrorKind};
pub use service::{Identity, Layer, Service, ServiceBuilder, Stack};
