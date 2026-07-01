//! `oath-adapter-net-api` — transport-neutral composition primitives + contracts.
//!
//! This crate is **std-only** (zero deps — the signal the ADR-0029 cut is
//! clean). It defines the shared abstractions every transport's layers depend
//! on:
//!
//! - [`compose`] — `Layer`, `ServiceBuilder`, `Identity`, `Stack`
//! - [`error_kind`] — `ErrorKind`, `HasErrorKind`
//! - [`timer`] — `Timer`
//!
//! `Service` is **not** here — it is a per-transport contract in
//! `oath-adapter-net-http-api` (ADR-0029 §2).
#![forbid(unsafe_code)]

pub mod compose;
pub mod error_kind;
pub mod timer;

pub use compose::{Identity, Layer, ServiceBuilder, Stack};
pub use error_kind::{ErrorKind, HasErrorKind};
pub use timer::Timer;
