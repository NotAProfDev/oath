//! Transport-neutral test doubles for the net adapter stack: a `MockTimer`
//! virtual clock beside the `Timer` contract in `oath-adapter-net-api`. Consumed
//! via `[dev-dependencies]` only — it has no production edge, so the HTTP and WS
//! stacks can fake the same clock without dev-depending on each other's mock.
#![forbid(unsafe_code)]

pub mod timer;

pub use timer::MockTimer;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state stays
/// usable so a failing test reports its own assertion, not a poison panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
