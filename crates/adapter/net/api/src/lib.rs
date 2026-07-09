//! `oath-adapter-net-api` — transport-neutral composition primitives + contracts.
//!
//! This crate is **std-only** (zero dependencies — the signal that the
//! transport-neutral cut is clean). It defines the shared abstractions every
//! transport's layers depend on:
//!
//! - [`compose`] — `Layer`, `LayerBuilder`, `Identity`, `Stack`
//! - [`error_kind`] — `ErrorKind`, `HasErrorKind`
//! - [`timer`] — `Timer`
#![forbid(unsafe_code)]

pub mod compose;
pub mod error_kind;
pub mod timer;

pub use compose::{Identity, Layer, LayerBuilder, Stack};
pub use error_kind::{ErrorKind, HasErrorKind};
pub use timer::Timer;
