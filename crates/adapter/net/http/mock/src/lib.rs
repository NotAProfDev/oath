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
