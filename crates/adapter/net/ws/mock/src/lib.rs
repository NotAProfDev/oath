//! Test harness for the net-ws stack: a scriptable `MockWsConnector` leaf
//! whose sources yield pre-set frames (or injected errors), whose sinks record
//! what was sent, and whose per-connection `LifecycleSender` a test can take
//! to drive lifecycle transitions. Consumed via `[dev-dependencies]` only —
//! it has no production edge. `MockTimer`/`MockSpawn` arrive with the
//! resilience slice (ADR-0033 §9).
#![forbid(unsafe_code)]

pub mod connector;
pub mod sink;
pub mod source;

pub use connector::MockWsConnector;
pub use sink::MockSink;
pub use source::MockSource;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `mutex`, recovering the guard if a panic poisoned it — mock state
/// stays usable so a failing test reports its own assertion, not a poison
/// panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
