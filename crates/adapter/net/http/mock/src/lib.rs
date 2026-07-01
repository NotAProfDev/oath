//! Test harness for the net-http stack: a canned-response `MockClient` leaf, a
//! frame-controllable `MockBody`, and a `MockTimer` virtual clock. Consumed by
//! downstream crates via `[dev-dependencies]` only — it has no production edge.
#![forbid(unsafe_code)]

pub mod body;
pub mod client;
pub mod timer;

pub use body::MockBody;
pub use client::MockClient;
pub use timer::MockTimer;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state stays
/// usable so a failing test reports its own assertion, not a poison panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
