//! `oath-adapter-net-ws-api` — the WebSocket transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`). Defines the WS transport contract (ADR-0032, as amended by
//! ADR-0033 §5): an untyped duplex frame channel — the transport moves frames
//! and knows nothing of venue grammar (subscriptions, topics, JSON), which
//! stays in the adapter (ADR-0003).
//!
//! - [`frame`] — the `Frame`/`CloseFrame` transport vocabulary
//!
//! The resilience stack (reconnect actor, heartbeat, buffer, `stack()`) and
//! the tungstenite backend land in later slices. No async runtime, `tokio`,
//! `tokio-tungstenite`, or `serde` here.
#![forbid(unsafe_code)]

pub mod frame;

pub use frame::{CloseFrame, Frame};
