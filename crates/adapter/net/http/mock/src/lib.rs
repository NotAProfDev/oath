//! Test harness for the net-http stack: a canned-response `MockClient` leaf and
//! a frame-controllable `MockBody`. Consumed by downstream crates via
//! `[dev-dependencies]` only — it has no production edge. (The `MockTimer`
//! virtual clock now lives in the transport-neutral `oath-adapter-net-mock`.)
#![forbid(unsafe_code)]

pub mod body;
pub mod client;

pub use body::MockBody;
pub use client::MockClient;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state stays
/// usable so a failing test reports its own assertion, not a poison panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
