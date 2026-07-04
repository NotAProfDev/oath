# net-ws WebSocket Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the new `oath-adapter-net-ws-api` crate its WebSocket data-plane contracts — `Frame`/`CloseFrame`, `WsError`, the split owned halves (`WsSink`/`WsSource`), the epoch-stamped lifecycle channel (`ConnState`/`LifecycleSnapshot`/`Lifecycle`), and the `WsConnector` leaf seam — and ship the standalone `oath-adapter-net-ws-mock` harness, so the resilience slice (reconnect actor, heartbeat, buffer, `stack()`) and the tungstenite leaf have a typed, mockable WS surface to build on.

**Architecture:** The contract is an **untyped duplex frame channel** (ADR-0032 §1): the transport moves RFC 6455 frames and knows nothing of venue grammar. The shape is **asymmetric** (§2) — recv is a `futures_core::Stream` of `Result<Frame, WsError>`; send is a one-shot RPITIT `send` (deliberately not `Sink`); `close(self)` consumes the sink so post-close sends are a compile error. `connect` yields **three** handles: the two single-owner halves plus `Lifecycle`, a last-value **watch of an epoch-stamped `LifecycleSnapshot`** (§4, resolved by ADR-0033 §5) — the out-of-band control plane the resilience layers will drive. `WsError` is one concrete error implementing `HasErrorKind`. Everything is compile-time `impl` seams — no `dyn`, no `async-trait`, no per-call alloc (ADR-0029 §5).

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `http`/`bytes`/`futures-core`/`async-watch`/`thiserror`. **No** `tokio`/`tokio-tungstenite`/`rustls`/`serde` — `net-ws-api` stays zero-I/O, zero-runtime.

**Source spec:** [ADR-0032](../../adr/0032-websocket-transport-contract-duplex-frames-lifecycle.md) (contract) as amended by [ADR-0033](../../adr/0033-websocket-resilience-reconnect-actor-watch-lifecycle.md) §5 (lifecycle = watch of `LifecycleSnapshot`; `Unrecoverable` in `ConnState`; `Lagged` as cumulative `total_lagged`). This is the WS twin of the HTTP contract slice ([2026-07-01-net-http-contract.md](2026-07-01-net-http-contract.md), PR #60).

**Explicitly deferred (later slices — do NOT build here):**
- `AuthSource` (ADR-0032 §8) — mirrors the HTTP workstream, where `AuthSource` is a later PR of the unmerged construction-surface spec; the trait must be *identical* in both crates, so WS declares it when it first consumes it (the reconnect layer).
- The whole ADR-0033 resilience stack: `Spawn`, the reconnect actor, heartbeat/liveness, the dual-bound buffer, `SendRateLimit`, `Tracing`, `stack()`, `WsConfig`, `ReconnectingConnector`/`ReconnectingConnection`/`WsControl` — and with it the `futures-util`/`pin-project-lite`/`tracing` production deps.
- The `oath-adapter-net-ws-tungstenite` leaf and `build()`.
- `MockTimer`/`MockSpawn` in the mock crate (needed only by the resilience slice; `MockTimer`'s home is also an open question of the net-http construction-surface spec).

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` (`unsafe_code = "deny"`; both new crates carry `#![forbid(unsafe_code)]`).
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result` / recover (`Mutex` poison via the `lock` helper pattern below). Test code is exempt for `unwrap`/`expect`/indexing only.
- **`just lint` runs clippy with `-D warnings`, promoting `pedantic`/`nursery` (warn-level) to errors** — all code **including tests** must be pedantic-clean: no truncating `as` casts (use `u64::try_from(x).unwrap_or(u64::MAX)`), `#[must_use]` where clippy asks, document all public items (`missing_docs`), `Debug` on every public type (`missing_debug_implementations`), no unreachable `pub`, `const fn` where nursery's `missing_const_for_fn` asks.
- **`net-ws-api` charter:** no `tokio`/`tokio-tungstenite`/`rustls`/`serde`; free of any async runtime. Production deps added by this slice: `oath-adapter-net-api`, `thiserror`, `bytes`, `futures-core`, `async-watch`, `http` — each added in the task that first *uses* it (keeps `cargo-machete` green). `futures-util`/`pin-project-lite`/`tracing` from the ADR-0032 dep list arrive with the resilience slice, not here.
- **Deps** via `[workspace.dependencies]` (explicit `version` for internal crates).
- **DoD for the PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete, …). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch (`feat/net-ws-contract`) → worktree under `.claude/worktrees/net-ws-contract` (never switch the primary checkout) → one PR (`Closes #<issue>`).
- **New external dep `async-watch` must clear `just deny`** (license/advisory). If it fails the gate, stop and surface — do not vendor or swap silently.

---

## File Structure

- `crates/adapter/net/ws/api/Cargo.toml` + `src/lib.rs` — **new crate** `oath-adapter-net-ws-api`.
- `crates/adapter/net/ws/api/src/frame.rs` — `Frame`, `CloseFrame`.
- `crates/adapter/net/ws/api/src/error.rs` — `WsError`, `BoxError`, `HasErrorKind` impl.
- `crates/adapter/net/ws/api/src/sink.rs` — `WsSink` (owned send half).
- `crates/adapter/net/ws/api/src/source.rs` — `WsSource` (owned recv half, blanket impl).
- `crates/adapter/net/ws/api/src/lifecycle.rs` — `ConnState`, `LifecycleSnapshot`, `Lifecycle`, `LifecycleSender`.
- `crates/adapter/net/ws/api/src/connector.rs` — `WsConnector`.
- `crates/adapter/net/ws/mock/{Cargo.toml,src/lib.rs,src/connector.rs,src/sink.rs,src/source.rs}` — **new crate** `oath-adapter-net-ws-mock`.
- `Cargo.toml` (workspace) — members + dep entries + `async-watch`/`futures-util` in `[workspace.dependencies]`.
- `README.md` — add `oath-adapter-net-ws-api` to the crate table + mermaid graph.
- `CHANGELOG.md` — `[Unreleased] → Added`.

Each task is one commit; the six tasks together are one PR/issue.

---

## Task 1: Crate skeleton + `Frame`/`CloseFrame`

**Files:**
- Create: `crates/adapter/net/ws/api/Cargo.toml`, `crates/adapter/net/ws/api/src/lib.rs`, `crates/adapter/net/ws/api/src/frame.rs`
- Modify: root `Cargo.toml` (member + dep entry)

**Interfaces:**
- Consumes: `bytes::Bytes`.
- Produces: `oath_adapter_net_ws_api::{Frame, CloseFrame}`. `Frame::{Text(Bytes), Binary(Bytes), Ping(Bytes), Pong(Bytes), Close(Option<CloseFrame>)}` (`Clone + PartialEq + Eq`); `Frame::is_data(&self) -> bool`, `Frame::is_control(&self) -> bool`. `CloseFrame { code: u16, reason: String }`.

- [ ] **Step 1: Register the crate**

Root `Cargo.toml`: add `"crates/adapter/net/ws/api",` to `members` (after the `net/http/mock` entry), and to `[workspace.dependencies]` (internal-crates block):

```toml
oath-adapter-net-ws-api = { path = "crates/adapter/net/ws/api", version = "0.1.0" }
```

Create `crates/adapter/net/ws/api/Cargo.toml`:

```toml
[package]
name = "oath-adapter-net-ws-api"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
bytes = { workspace = true }
```

Create `crates/adapter/net/ws/api/src/lib.rs`:

```rust
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
```

(The `//!` module list grows one bullet per task; later tasks say "add the module-doc line".)

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/ws/api/src/frame.rs` with only the test:

```rust
//! The WebSocket frame vocabulary — placeholder; filled in step 4.

#[cfg(test)]
mod tests {
    use super::{CloseFrame, Frame};
    use bytes::Bytes;

    #[test]
    fn data_and_control_frames_classify() {
        assert!(Frame::Text(Bytes::from_static(b"{}")).is_data());
        assert!(Frame::Binary(Bytes::new()).is_data());
        assert!(Frame::Ping(Bytes::new()).is_control());
        assert!(Frame::Pong(Bytes::new()).is_control());
        assert!(Frame::Close(None).is_control());
    }

    #[test]
    fn frames_are_cloneable_and_comparable() {
        let close = Frame::Close(Some(CloseFrame { code: 1000, reason: "bye".to_owned() }));
        assert_eq!(close.clone(), close);
        assert_ne!(close, Frame::Close(None));
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Frame` / `CloseFrame`.

- [ ] **Step 4: Implement `Frame` + `CloseFrame`**

Prepend to `frame.rs` (replacing the placeholder `//!` line):

```rust
//! The WebSocket frame vocabulary — the leaf/inter-layer unit of transport.
//!
//! "Untyped" (ADR-0032 §1) means *no venue/JSON typing* — not flattening
//! WebSocket's own protocol frame kinds, which are transport concerns
//! (RFC 6455), not venue concerns (§3). After the ADR-0033 default stack the
//! adapter-facing source delivers only `Text`/`Binary` data frames; control
//! frames are absorbed by the heartbeat layer and bypass the data buffer.

use bytes::Bytes;

/// The payload of a `Close` frame: an RFC 6455 close code plus a UTF-8 reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    /// The RFC 6455 §7.4 close code (e.g. `1000` normal closure).
    pub code: u16,
    /// The UTF-8 close reason; may be empty.
    pub reason: String,
}

/// One WebSocket frame. Payloads are raw [`Bytes`] — the transport never
/// parses them (grammar-blindness, ADR-0032 §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A UTF-8 text data frame (kept as bytes; the adapter owns parsing).
    Text(Bytes),
    /// A binary data frame.
    Binary(Bytes),
    /// A protocol ping control frame.
    Ping(Bytes),
    /// A protocol pong control frame.
    Pong(Bytes),
    /// A close control frame with an optional code + reason.
    Close(Option<CloseFrame>),
}

impl Frame {
    /// Whether this is a data frame (`Text`/`Binary`) — what the default
    /// stack delivers to the adapter (ADR-0032 §3).
    #[must_use]
    pub const fn is_data(&self) -> bool {
        matches!(self, Self::Text(_) | Self::Binary(_))
    }

    /// Whether this is a control frame (`Ping`/`Pong`/`Close`) — absorbed by
    /// the heartbeat layer; bypasses the data buffer (ADR-0032 §3/§6).
    #[must_use]
    pub const fn is_control(&self) -> bool {
        !self.is_data()
    }
}
```

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-ws-api frame && just lint`
Expected: PASS, warning-free.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/ws/api Cargo.toml
git commit -m "feat(net): net-ws-api skeleton + Frame/CloseFrame vocabulary"
```

---

## Task 2: `WsError` + `HasErrorKind`

**Files:**
- Create: `crates/adapter/net/ws/api/src/error.rs`
- Modify: `crates/adapter/net/ws/api/src/lib.rs`, `crates/adapter/net/ws/api/Cargo.toml`

**Interfaces:**
- Consumes: `oath_adapter_net_api::{ErrorKind, HasErrorKind}`; `crate::CloseFrame`.
- Produces: `oath_adapter_net_ws_api::{WsError, BoxError}`. Variants `Timeout`, `Connection(BoxError)`, `Auth(String)`, `Closed(Option<CloseFrame>)`, `Other(BoxError)`. Constructors `WsError::connection(impl Into<BoxError>)`, `WsError::auth(impl Into<String>)`, `WsError::other(impl Into<BoxError>)`. `impl HasErrorKind for WsError`.

- [ ] **Step 1: Add deps**

Append to `crates/adapter/net/ws/api/Cargo.toml` `[dependencies]`:

```toml
oath-adapter-net-api = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/ws/api/src/error.rs` with only the test, and add `pub mod error;` + `pub use error::{BoxError, WsError};` + the module-doc line (`//! - [\`error\`] — \`WsError\` and its \`HasErrorKind\` impl`) to `lib.rs`:

```rust
//! The single concrete error type for the WS stack — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::WsError;
    use crate::CloseFrame;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(WsError::Timeout.kind(), ErrorKind::Timeout);
        assert_eq!(WsError::connection("reset").kind(), ErrorKind::Connection);
        assert_eq!(WsError::auth("expired").kind(), ErrorKind::Auth);
        assert_eq!(WsError::Closed(None).kind(), ErrorKind::Connection);
        let close = CloseFrame { code: 1000, reason: String::new() };
        assert_eq!(WsError::Closed(Some(close)).kind(), ErrorKind::Connection);
        assert_eq!(WsError::other("boom").kind(), ErrorKind::Unknown);
    }

    #[test]
    fn auth_carries_message() {
        assert_eq!(
            WsError::auth("session expired").to_string(),
            "authorization failed: session expired"
        );
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WsError>();
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type WsError`.

- [ ] **Step 4: Implement `WsError`**

Prepend to `error.rs`:

```rust
//! The single concrete error type across the WS stack — transport and
//! middleware failures only. Venue-level errors arriving as data frames are
//! NOT errors here: they flow through the source as `Text`/`Binary` for the
//! adapter to classify (ADR-0032 §1). `WsError` implements [`HasErrorKind`]
//! once; the resilience layers branch only on [`ErrorKind`] (ADR-0033 §7).

use crate::CloseFrame;
use oath_adapter_net_api::{ErrorKind, HasErrorKind};

/// A boxed error source, preserving backend detail for logs without leaking
/// the concrete type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The single error of the WS transport: `connect`, `send`, `close`, and every
/// source item use it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsError {
    /// The operation did not complete within its timeout.
    #[error("operation timed out")]
    Timeout,
    /// A connection-level failure (DNS, TCP, TLS, WS handshake or protocol).
    #[error("connection failure")]
    Connection(#[source] BoxError),
    /// Credential stamping or refresh failed.
    #[error("authorization failed: {0}")]
    Auth(String),
    /// The peer closed the connection (close frame, possibly with code+reason).
    #[error("connection closed by peer")]
    Closed(Option<CloseFrame>),
    /// A backend error that does not fit another variant.
    #[error("websocket error")]
    Other(#[source] BoxError),
}

impl WsError {
    /// Construct a [`WsError::Connection`] from a source error.
    #[must_use]
    pub fn connection(source: impl Into<BoxError>) -> Self {
        Self::Connection(source.into())
    }

    /// Construct a [`WsError::Auth`] from a message.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    /// Construct a [`WsError::Other`] from a source error.
    #[must_use]
    pub fn other(source: impl Into<BoxError>) -> Self {
        Self::Other(source.into())
    }
}

impl HasErrorKind for WsError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Timeout => ErrorKind::Timeout,
            // A peer close is a connection-level loss for retry purposes;
            // per-close-code refinement is the adapter's classification hook
            // (ADR-0033 §7), not a transport concern.
            Self::Connection(_) | Self::Closed(_) => ErrorKind::Connection,
            Self::Auth(_) => ErrorKind::Auth,
            Self::Other(_) => ErrorKind::Unknown,
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-ws-api error && just lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/ws/api/src/error.rs crates/adapter/net/ws/api/src/lib.rs crates/adapter/net/ws/api/Cargo.toml
git commit -m "feat(net): WsError — one concrete transport error, HasErrorKind"
```

---

## Task 3: `WsSink` + `WsSource` — the split owned halves

**Files:**
- Create: `crates/adapter/net/ws/api/src/sink.rs`, `crates/adapter/net/ws/api/src/source.rs`
- Modify: `crates/adapter/net/ws/api/src/lib.rs`, `crates/adapter/net/ws/api/Cargo.toml`, root `Cargo.toml`

**Interfaces:**
- Consumes: `crate::{Frame, WsError}`; `futures_core::Stream`.
- Produces: `oath_adapter_net_ws_api::WsSink` — `trait WsSink: Send` with `fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send` and `fn close(self) -> impl Future<Output = Result<(), WsError>> + Send`. `oath_adapter_net_ws_api::WsSource` — `trait WsSource: Stream<Item = Result<Frame, WsError>> + Send {}` with blanket `impl<S> WsSource for S where S: Stream<Item = Result<Frame, WsError>> + Send`.

- [ ] **Step 1: Add deps**

Root `Cargo.toml` `[workspace.dependencies]` (external block) — add `futures-util` (dev-use here; the resilience slice promotes it to a production dep per ADR-0032):

```toml
futures-util = "0.3"
```

(`futures-core` is already a workspace dependency.)

`crates/adapter/net/ws/api/Cargo.toml` — append to `[dependencies]` and add the dev block:

```toml
futures-core = { workspace = true }
```

```toml
[dev-dependencies]
tokio = { workspace = true }
futures-util = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/ws/api/src/sink.rs` with only the test; add `pub mod sink;` + `pub use sink::WsSink;` + module-doc line to `lib.rs`:

```rust
//! The owned send half — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::WsSink;
    use crate::{Frame, WsError};
    use bytes::Bytes;
    use std::future::Future;

    /// Inline double: records sent frames, succeeds on close.
    #[derive(Default)]
    struct VecSink {
        sent: Vec<Frame>,
    }

    impl WsSink for VecSink {
        fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
            self.sent.push(frame);
            std::future::ready(Ok(()))
        }

        fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn send_is_one_shot_and_close_consumes_the_sink() {
        let mut sink = VecSink::default();
        sink.send(Frame::Text(Bytes::from_static(b"smd+265598+{}"))).await.unwrap();
        assert_eq!(sink.sent.len(), 1);
        // `close(self)` takes the sink by value — `sink.send(...)` after this
        // line would be a compile error (ADR-0032 §2's type-system guarantee).
        sink.close().await.unwrap();
    }
}
```

Create `crates/adapter/net/ws/api/src/source.rs` with only the test; add `pub mod source;` + `pub use source::WsSource;` + module-doc line to `lib.rs`:

```rust
//! The owned recv half — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::WsSource;
    use crate::{Frame, WsError};
    use bytes::Bytes;
    use futures_core::Stream;
    use futures_util::StreamExt;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// Inline double: yields scripted items, then ends.
    struct ScriptSource {
        items: VecDeque<Result<Frame, WsError>>,
    }

    impl Stream for ScriptSource {
        type Item = Result<Frame, WsError>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            // No pinned fields — `get_mut` is sound (auto-`Unpin`).
            Poll::Ready(self.get_mut().items.pop_front())
        }
    }

    #[test]
    fn any_matching_stream_is_a_ws_source() {
        fn assert_ws_source<S: WsSource>(_: &S) {}
        let source = ScriptSource { items: VecDeque::new() };
        assert_ws_source(&source); // blanket impl applies
    }

    #[tokio::test]
    async fn source_yields_items_then_ends() {
        let mut source = ScriptSource {
            items: VecDeque::from([
                Ok(Frame::Text(Bytes::from_static(b"{\"topic\":\"system\"}"))),
                Err(WsError::connection("reset")),
            ]),
        };
        assert!(source.next().await.unwrap().is_ok());
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find trait WsSink` / `WsSource`.

- [ ] **Step 4: Implement the two traits**

Prepend to `sink.rs`:

```rust
//! The owned send half: one-shot RPITIT `send`, terminal `close`.
//!
//! Deliberately **not** `futures::Sink` — its `poll_ready`/`start_send`/
//! `poll_flush` is the poll-handshake the `Service` design walked away from
//! (ADR-0032 §2); subscribe/heartbeat traffic is low-volume, so a one-shot
//! `send` suffices. The half is single-owner and moves to its own task.

use crate::{Frame, WsError};
use std::future::Future;

/// The send half of one WebSocket connection.
///
/// `Send` because the halves move to separate tasks (concurrent send of
/// subscribe/heartbeat vs. receive of frames). `'static` is not required
/// here — it is enforced at the composition boundary, as for `Service`.
pub trait WsSink: Send {
    /// Send one frame.
    fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send;

    /// Initiate the closing handshake. Consumes the sink — shutdown is one-way
    /// and terminal, so the sink cannot be used after close is requested
    /// (enforced by the type system, ADR-0032 §2).
    fn close(self) -> impl Future<Output = Result<(), WsError>> + Send;
}
```

Prepend to `source.rs`:

```rust
//! The owned recv half: a stream of frames.
//!
//! Recv is `futures_core::Stream`, not a hand-rolled pull iterator — the
//! `http_body::Body` precedent (ADR-0032 §2): monomorphised, zero-box, and it
//! gives the resilience layers `StreamExt`/`unfold` instead of manual
//! `poll_next`. Exclusive `&mut` access is inherent in `Stream::poll_next`.

use crate::{Frame, WsError};
use futures_core::Stream;

/// The receive half of one WebSocket connection: frames in arrival order,
/// each `Ok(Frame)` or a terminal-ish `Err(WsError)`.
///
/// Blanket-implemented for every matching stream — a backend implements
/// `Stream` once and is a `WsSource` for free (the `HttpClient` move).
pub trait WsSource: Stream<Item = Result<Frame, WsError>> + Send {}

impl<S> WsSource for S where S: Stream<Item = Result<Frame, WsError>> + Send {}
```

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-ws-api sink source && just lint`

(Note: `cargo test -p oath-adapter-net-ws-api sink source` runs tests matching either filter only if given as one filter word; if it errors, run `cargo test -p oath-adapter-net-ws-api` for the whole crate.)
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/ws/api/src/sink.rs crates/adapter/net/ws/api/src/source.rs crates/adapter/net/ws/api/src/lib.rs crates/adapter/net/ws/api/Cargo.toml Cargo.toml
git commit -m "feat(net): WsSink/WsSource — the split owned connection halves"
```

---

## Task 4: Lifecycle — `ConnState`, `LifecycleSnapshot`, watch pair

**Files:**
- Create: `crates/adapter/net/ws/api/src/lifecycle.rs`
- Modify: `crates/adapter/net/ws/api/src/lib.rs`, `crates/adapter/net/ws/api/Cargo.toml`, root `Cargo.toml`

**Interfaces:**
- Consumes: `async-watch` (channel/borrow/changed/send), `std::time::Instant`.
- Produces: `oath_adapter_net_ws_api::{ConnState, LifecycleSnapshot, Lifecycle, LifecycleSender}`.
  - `ConnState` (`Copy`): `Connected { epoch: u64 }`, `Stale`, `Reconnecting`, `Resumed { epoch: u64 }`, `Unrecoverable`.
  - `LifecycleSnapshot` (`Copy`, plain data): `phase: ConnState`, `epoch: u64`, `down_since: Option<Instant>`, `attempts: u64`, `total_lagged: u64`; `LifecycleSnapshot::connected(epoch: u64) -> Self`.
  - `Lifecycle` (`Clone`): `Lifecycle::channel(initial: LifecycleSnapshot) -> (LifecycleSender, Lifecycle)`, `Lifecycle::snapshot(&self) -> LifecycleSnapshot`, `async Lifecycle::changed(&mut self) -> bool`.
  - `LifecycleSender`: `send(&self, LifecycleSnapshot)` (overwrite; never blocks).

**Known risk:** `async-watch` 0.3's exact API (`async_watch::channel(initial) -> (Sender, Receiver)`, `Receiver::borrow()`, `Receiver::changed(&mut self) -> Result<(), _>`, `Receiver: Clone`, `Sender::send(&self, T) -> Result<(), _>`) mirrors `tokio::sync::watch` but **verify at the failing-test step** — the wrapper internals may adjust; the public `Lifecycle`/`LifecycleSender` API above is what is fixed. Also confirm `just deny` accepts the new dep.

- [ ] **Step 1: Add deps**

Root `Cargo.toml` `[workspace.dependencies]` (external block):

```toml
# Runtime-neutral last-value channel (extracted from tokio::sync::watch,
# event-listener family) — keeps tokio out of net-ws-api (ADR-0033 §5).
async-watch = "0.3"
```

`crates/adapter/net/ws/api/Cargo.toml` `[dependencies]`:

```toml
async-watch = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/ws/api/src/lifecycle.rs` with only the test; add `pub mod lifecycle;` + `pub use lifecycle::{ConnState, Lifecycle, LifecycleSender, LifecycleSnapshot};` + module-doc line to `lib.rs`:

```rust
//! The epoch-stamped lifecycle channel — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::{ConnState, Lifecycle, LifecycleSnapshot};

    #[test]
    fn connected_seeds_a_consistent_snapshot() {
        let snap = LifecycleSnapshot::connected(3);
        assert_eq!(snap.phase, ConnState::Connected { epoch: 3 });
        assert_eq!(snap.epoch, 3);
        assert_eq!(snap.down_since, None);
        assert_eq!(snap.attempts, 0);
        assert_eq!(snap.total_lagged, 0);
    }

    #[test]
    fn snapshot_reads_the_latest_value_without_blocking() {
        let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
        assert_eq!(lifecycle.snapshot().phase, ConnState::Connected { epoch: 0 });
        tx.send(LifecycleSnapshot { phase: ConnState::Stale, ..LifecycleSnapshot::connected(0) });
        assert_eq!(lifecycle.snapshot().phase, ConnState::Stale);
    }

    #[test]
    fn overwrite_coalesces_but_the_epoch_carries_the_cycle_count() {
        // ADR-0033 §5: a slow consumer may miss transient phases, but
        // { currently-down ∨ epoch-advanced } is total — the epoch delta
        // recovers fully-coalesced down-cycles.
        let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(5));
        tx.send(LifecycleSnapshot { phase: ConnState::Reconnecting, ..LifecycleSnapshot::connected(5) });
        tx.send(LifecycleSnapshot::connected(9)); // several cycles later
        let seen = lifecycle.snapshot();
        assert_eq!(seen.phase, ConnState::Connected { epoch: 9 });
        assert_eq!(seen.epoch - 5, 4); // four down-cycles, regardless of what was witnessed
    }

    #[tokio::test]
    async fn changed_wakes_on_update_and_ends_when_the_sender_drops() {
        let (tx, mut lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
        tx.send(LifecycleSnapshot { phase: ConnState::Reconnecting, ..LifecycleSnapshot::connected(0) });
        assert!(lifecycle.changed().await); // pending update is observed
        assert_eq!(lifecycle.snapshot().phase, ConnState::Reconnecting);
        drop(tx);
        assert!(!lifecycle.changed().await); // producer gone — no more updates
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Lifecycle` etc.

- [ ] **Step 4: Implement the lifecycle channel**

Prepend to `lifecycle.rs`:

```rust
//! The out-of-band lifecycle channel: connection health as a third handle,
//! not an item interleaved into the data stream (ADR-0032 §4).
//!
//! Delivery is a **last-value watch of an epoch-stamped snapshot** (ADR-0033
//! §5): the producer overwrites and never blocks, so a slow risk consumer can
//! never backpressure the socket-owning actor; the monotonic `epoch` makes
//! coalescing lossless for the safety fact ({ currently-down ∨ epoch-advanced }
//! is total). Every snapshot field is **level or monotonic-cumulative, never a
//! per-event delta** — overwrite semantics would lose a delta.

use std::fmt;
use std::time::Instant;

/// The connection phase — the level component of [`LifecycleSnapshot`].
///
/// `Stale`/`Reconnecting` are first-class: for a trading system the
/// safety-critical event is the feed going *down*, not its recovery
/// (ADR-0032 §4). `Unrecoverable` is the one terminal phase (ADR-0033 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnState {
    /// The connection is up. `epoch` echoes [`LifecycleSnapshot::epoch`].
    Connected {
        /// The connection epoch at the time of this phase.
        epoch: u64,
    },
    /// The feed is stale (idle-read timeout / missed liveness) — treat as down.
    Stale,
    /// The connection is down; the reconnect actor is re-establishing it.
    Reconnecting,
    /// A reconnect completed; `epoch` bounds the adapter's reconcile window
    /// ("reconcile since epoch N", ADR-0032 §4/§5).
    Resumed {
        /// The new connection epoch after the completed down-cycle.
        epoch: u64,
    },
    /// A classified permanent failure — the stack has stopped retrying and
    /// will not self-heal (ADR-0033 §7).
    Unrecoverable,
}

/// One epoch-stamped, level-semantics view of connection health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    /// The current connection phase (level).
    pub phase: ConnState,
    /// Monotonic; bumped on every completed down-cycle. The canonical source
    /// of truth — the value echoed in `Connected`/`Resumed` is this field;
    /// consumers diff it.
    pub epoch: u64,
    /// When the current down phase began (`None` while up).
    pub down_since: Option<Instant>,
    /// Monotonic count of connection attempts.
    pub attempts: u64,
    /// Monotonic cumulative count of frames dropped by the buffer layer — the
    /// `Lagged` signal (ADR-0032 §6); a per-event delta would be lost to
    /// overwrite, so consumers diff this total.
    pub total_lagged: u64,
}

impl LifecycleSnapshot {
    /// A healthy just-connected snapshot at `epoch`.
    #[must_use]
    pub const fn connected(epoch: u64) -> Self {
        Self {
            phase: ConnState::Connected { epoch },
            epoch,
            down_since: None,
            attempts: 0,
            total_lagged: 0,
        }
    }
}

/// The read side of the lifecycle channel — the third handle `connect` yields.
///
/// Cloneable: risk loop and adapter each hold their own cursor.
#[derive(Clone)]
pub struct Lifecycle {
    rx: async_watch::Receiver<LifecycleSnapshot>,
}

impl Lifecycle {
    /// Create a linked producer/consumer pair seeded with `initial`.
    #[must_use]
    pub fn channel(initial: LifecycleSnapshot) -> (LifecycleSender, Self) {
        let (tx, rx) = async_watch::channel(initial);
        (LifecycleSender { tx }, Self { rx })
    }

    /// The latest snapshot — a level read; never blocks.
    #[must_use]
    pub fn snapshot(&self) -> LifecycleSnapshot {
        *self.rx.borrow()
    }

    /// Wait until a snapshot newer than the last one seen by *this* handle is
    /// published. Returns `false` once the producer is dropped (terminal — no
    /// further updates will ever arrive).
    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }
}

impl fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycle").field("snapshot", &self.snapshot()).finish()
    }
}

/// The write side, held by the connection owner (a leaf, the reconnect actor,
/// or a mock). `send` overwrites — it never blocks on a slow consumer.
pub struct LifecycleSender {
    tx: async_watch::Sender<LifecycleSnapshot>,
}

impl LifecycleSender {
    /// Publish a new snapshot. Best-effort: sending with every receiver gone
    /// is a no-op, not a failure — the producer never depends on consumers.
    pub fn send(&self, snapshot: LifecycleSnapshot) {
        // Err means all receivers dropped — nothing to notify.
        let _ = self.tx.send(snapshot);
    }
}

impl fmt::Debug for LifecycleSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifecycleSender").finish_non_exhaustive()
    }
}
```

(If `async_watch::Receiver` turns out not to be `Clone`, drop the `#[derive(Clone)]`-equivalent on `Lifecycle`, note it in the PR, and file the follow-up — do not hand-roll a watch here.)

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-ws-api lifecycle && just lint && just deny`
Expected: PASS; `deny` accepts `async-watch`.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/ws/api/src/lifecycle.rs crates/adapter/net/ws/api/src/lib.rs crates/adapter/net/ws/api/Cargo.toml Cargo.toml
git commit -m "feat(net): lifecycle channel — ConnState, LifecycleSnapshot, watch pair"
```

---

## Task 5: `WsConnector` — the leaf seam

**Files:**
- Create: `crates/adapter/net/ws/api/src/connector.rs`
- Modify: `crates/adapter/net/ws/api/src/lib.rs`, `crates/adapter/net/ws/api/Cargo.toml`

**Interfaces:**
- Consumes: `crate::{Lifecycle, LifecycleSnapshot, WsError, WsSink, WsSource}`; `http`.
- Produces: `oath_adapter_net_ws_api::WsConnector` — `type Sink: WsSink`, `type Source: WsSource`, `fn connect(&self, handshake: http::Request<()>) -> impl Future<Output = Result<(Self::Sink, Self::Source, Lifecycle), WsError>> + Send`.

- [ ] **Step 1: Add deps**

Append to `crates/adapter/net/ws/api/Cargo.toml` `[dependencies]`:

```toml
http = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/ws/api/src/connector.rs` with only the test; add `pub mod connector;` + `pub use connector::WsConnector;` + module-doc line to `lib.rs`:

```rust
//! The `WsConnector` dependency-inversion seam — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::WsConnector;
    use crate::{Frame, Lifecycle, LifecycleSnapshot, WsError, WsSink};
    use futures_core::Stream;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct StubSink;
    impl WsSink for StubSink {
        fn send(&mut self, _frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
        fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    struct StubSource;
    impl Stream for StubSource {
        type Item = Result<Frame, WsError>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    struct StubConnector;
    impl WsConnector for StubConnector {
        type Sink = StubSink;
        type Source = StubSource;
        fn connect(
            &self,
            _handshake: http::Request<()>,
        ) -> impl Future<Output = Result<(StubSink, StubSource, Lifecycle), WsError>> + Send
        {
            async {
                let (_tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
                Ok((StubSink, StubSource, lifecycle))
            }
        }
    }

    #[tokio::test]
    async fn connect_yields_the_three_handles() {
        fn assert_connector<C: WsConnector>(_: &C) {}
        assert_connector(&StubConnector);

        let mut handshake = http::Request::new(());
        *handshake.uri_mut() = "wss://api.ibkr.com/v1/api/ws".parse().unwrap();
        let (sink, _source, lifecycle) = StubConnector.connect(handshake).await.unwrap();
        assert_eq!(lifecycle.snapshot().epoch, 0);
        sink.close().await.unwrap();
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find trait WsConnector`.

- [ ] **Step 4: Implement `WsConnector`**

Prepend to `connector.rs`:

```rust
//! The named dependency-inversion seam the composition stack builds on — the
//! `HttpClient` analogue for WS (ADR-0032 §7). The WS upgrade *is* an HTTP
//! GET, so the handshake is an `http::Request<()>` (body-agnostic parts —
//! the same shape the shared `AuthSource` will stamp, §8). Per ADR-0029 §5 it
//! is a compile-time `impl WsConnector` seam — never `dyn`.
//!
//! This is the **composition** seam: the leaf and every inner resilience
//! layer implement it (ADR-0033 §1). The richer usage seam an adapter holds
//! (`ReconnectingConnector`/`ReconnectingConnection` + `WsControl`) is
//! produced only at the assembly boundary, in a later slice.

use crate::{Lifecycle, WsError, WsSink, WsSource};
use std::future::Future;

/// Establish WebSocket connections: one handshake in, three handles out.
pub trait WsConnector {
    /// The send half produced by a successful connect.
    type Sink: WsSink;
    /// The receive half produced by a successful connect.
    type Source: WsSource;

    /// Perform the upgrade handshake and yield the two single-owner halves
    /// plus the lifecycle read channel (ADR-0032 §2/§4). `&self` — a connector
    /// is shared; the reconnect layer calls it once per (re)connect.
    fn connect(
        &self,
        handshake: http::Request<()>,
    ) -> impl Future<Output = Result<(Self::Sink, Self::Source, Lifecycle), WsError>> + Send;
}
```

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-ws-api connector && just lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/ws/api/src/connector.rs crates/adapter/net/ws/api/src/lib.rs crates/adapter/net/ws/api/Cargo.toml
git commit -m "feat(net): WsConnector leaf seam — handshake in, three handles out"
```

---

## Task 6: `oath-adapter-net-ws-mock` harness + README + CHANGELOG

**Files:**
- Create: `crates/adapter/net/ws/mock/Cargo.toml`, `.../src/lib.rs`, `.../src/connector.rs`, `.../src/sink.rs`, `.../src/source.rs`
- Modify: root `Cargo.toml` (member + dep entry), `README.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: `oath_adapter_net_ws_api::{Frame, Lifecycle, LifecycleSender, LifecycleSnapshot, WsConnector, WsError, WsSink}`; `http`, `futures_core`.
- Produces: `oath_adapter_net_ws_mock::{MockWsConnector, MockSink, MockSource}`.
  - `MockWsConnector::new()`, `script_connection(&self, impl IntoIterator<Item = Result<Frame, WsError>>)`, `script_connect_error(&self, WsError)`, `connect_count(&self) -> usize`, `recorded_handshakes(&self) -> Vec<http::Request<()>>`, `sent_frames(&self, connection: usize) -> Vec<Frame>`, `close_called(&self, connection: usize) -> bool`, `take_lifecycle_sender(&self, connection: usize) -> Option<LifecycleSender>`.
  - Connection `n` (0-based) gets `Lifecycle` seeded `LifecycleSnapshot::connected(n as epoch)`; an unscripted `connect` succeeds with an immediately-ended source.

*(Standalone harness — `net-ws-api` does NOT depend on it, so there is no dev-dep cycle. The resilience slice consumes it via `[dev-dependencies]` and will extend it — e.g. injectable `send` failures — when it needs them; YAGNI here.)*

- [ ] **Step 1: Register the crate**

Root `Cargo.toml`: add `"crates/adapter/net/ws/mock",` to `members` (after `net/ws/api`), and to `[workspace.dependencies]`:

```toml
oath-adapter-net-ws-mock = { path = "crates/adapter/net/ws/mock", version = "0.1.0" }
```

Create `crates/adapter/net/ws/mock/Cargo.toml`:

```toml
[package]
name = "oath-adapter-net-ws-mock"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
oath-adapter-net-ws-api = { workspace = true }
http = { workspace = true }
futures-core = { workspace = true }

[dev-dependencies]
oath-adapter-net-api = { workspace = true }
bytes = { workspace = true }
tokio = { workspace = true }
futures-util = { workspace = true }
```

Create `crates/adapter/net/ws/mock/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: `MockSource` + `MockSink` — write the failing tests**

Create `crates/adapter/net/ws/mock/src/source.rs`:

```rust
//! The scripted receive half — placeholder; step 3.

#[cfg(test)]
mod tests {
    use super::MockSource;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use oath_adapter_net_ws_api::{Frame, WsError, WsSource};
    use std::collections::VecDeque;

    #[tokio::test]
    async fn yields_scripted_items_in_order_then_ends() {
        fn assert_ws_source<S: WsSource>(_: &S) {}
        let mut source = MockSource::new(VecDeque::from([
            Ok(Frame::Text(Bytes::from_static(b"{\"topic\":\"system\",\"hb\":1}"))),
            Err(WsError::connection("reset")),
        ]));
        assert_ws_source(&source);
        assert_eq!(
            source.next().await.unwrap().unwrap(),
            Frame::Text(Bytes::from_static(b"{\"topic\":\"system\",\"hb\":1}"))
        );
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());
    }
}
```

Create `crates/adapter/net/ws/mock/src/sink.rs`:

```rust
//! The recording send half — placeholder; step 3.

#[cfg(test)]
mod tests {
    use super::{ConnectionRecord, MockSink};
    use bytes::Bytes;
    use oath_adapter_net_ws_api::{Frame, WsSink};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn records_sends_and_close() {
        let record = Arc::new(Mutex::new(ConnectionRecord::default()));
        let mut sink = MockSink::new(Arc::clone(&record));
        sink.send(Frame::Text(Bytes::from_static(b"tic"))).await.unwrap();
        sink.close().await.unwrap();
        let record = record.lock().unwrap();
        assert_eq!(record.sent, vec![Frame::Text(Bytes::from_static(b"tic"))]);
        assert!(record.closed);
    }
}
```

- [ ] **Step 3: Implement `MockSource` + `MockSink`**

Prepend to `source.rs`:

```rust
//! The receive half a [`crate::MockWsConnector`] yields: pops its scripted
//! items in order, then ends the stream (connection over).

use futures_core::Stream;
use oath_adapter_net_ws_api::{Frame, WsError};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A scripted receive half. Satisfies `WsSource` via the blanket impl.
#[derive(Debug)]
pub struct MockSource {
    items: VecDeque<Result<Frame, WsError>>,
}

impl MockSource {
    /// A source that yields `items` in order, then ends.
    #[must_use]
    pub fn new(items: VecDeque<Result<Frame, WsError>>) -> Self {
        Self { items }
    }
}

impl Stream for MockSource {
    type Item = Result<Frame, WsError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // No pinned fields — `get_mut` is sound (auto-`Unpin`).
        Poll::Ready(self.get_mut().items.pop_front())
    }
}
```

Prepend to `sink.rs`:

```rust
//! The send half a [`crate::MockWsConnector`] yields: records every frame and
//! the close into the per-connection record the connector shares.

use crate::lock;
use oath_adapter_net_ws_api::{Frame, LifecycleSender, WsError, WsSink};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// What one mock connection observed. Shared between the [`MockSink`] handed
/// to the code under test and the [`crate::MockWsConnector`]'s accessors.
#[derive(Debug, Default)]
pub(crate) struct ConnectionRecord {
    /// Frames sent through the sink, in order.
    pub(crate) sent: Vec<Frame>,
    /// Whether `close` was called.
    pub(crate) closed: bool,
    /// The lifecycle write side, until a test `take`s it.
    pub(crate) lifecycle: Option<LifecycleSender>,
}

/// A recording send half. Sends always succeed.
#[derive(Debug)]
pub struct MockSink {
    record: Arc<Mutex<ConnectionRecord>>,
}

impl MockSink {
    pub(crate) fn new(record: Arc<Mutex<ConnectionRecord>>) -> Self {
        Self { record }
    }
}

impl WsSink for MockSink {
    fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), WsError>> + Send {
        lock(&self.record).sent.push(frame);
        std::future::ready(Ok(()))
    }

    fn close(self) -> impl Future<Output = Result<(), WsError>> + Send {
        lock(&self.record).closed = true;
        std::future::ready(Ok(()))
    }
}
```

Run: `just check && cargo test -p oath-adapter-net-ws-mock && just lint`
Expected: PASS. (`MockWsConnector` doesn't exist yet — `lib.rs` still lists `pub mod connector;`, so create `connector.rs` with just its placeholder `//!` line and the test from step 4 when you get there; if you prefer, comment nothing out and do steps 2–3 with the `connector` module line deferred to step 4.)

- [ ] **Step 4: `MockWsConnector` — write the failing test**

Create `crates/adapter/net/ws/mock/src/connector.rs`:

```rust
//! The scriptable connector leaf — placeholder; step 5.

#[cfg(test)]
mod tests {
    use super::MockWsConnector;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};
    use oath_adapter_net_ws_api::{
        ConnState, Frame, LifecycleSnapshot, WsConnector, WsError, WsSink,
    };

    fn handshake(uri: &str) -> http::Request<()> {
        let mut req = http::Request::new(());
        *req.uri_mut() = uri.parse().unwrap();
        req
    }

    #[tokio::test]
    async fn scripted_frames_flow_and_everything_is_recorded() {
        let connector = MockWsConnector::new();
        connector.script_connection([
            Ok(Frame::Text(Bytes::from_static(b"{\"topic\":\"smd+265598\"}"))),
            Err(WsError::connection("reset")),
        ]);

        let (mut sink, mut source, lifecycle) =
            connector.connect(handshake("wss://api.ibkr.com/v1/api/ws")).await.unwrap();

        assert_eq!(lifecycle.snapshot().phase, ConnState::Connected { epoch: 0 });
        assert!(source.next().await.unwrap().is_ok());
        assert!(source.next().await.unwrap().is_err());
        assert!(source.next().await.is_none());

        sink.send(Frame::Text(Bytes::from_static(b"smd+265598+{}"))).await.unwrap();
        sink.close().await.unwrap();

        assert_eq!(connector.connect_count(), 1);
        assert_eq!(connector.recorded_handshakes()[0].uri(), "wss://api.ibkr.com/v1/api/ws");
        assert_eq!(connector.sent_frames(0), vec![Frame::Text(Bytes::from_static(b"smd+265598+{}"))]);
        assert!(connector.close_called(0));
    }

    #[tokio::test]
    async fn scripted_connect_error_fails_then_unscripted_connect_succeeds_empty() {
        let connector = MockWsConnector::new();
        connector.script_connect_error(WsError::auth("session expired"));

        let err = connector.connect(handshake("wss://x/ws")).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
        assert_eq!(connector.connect_count(), 1); // the failed attempt is counted

        // No script left: connect succeeds with an immediately-ended source,
        // and epochs number successful connections (this is epoch 0).
        let (_sink, mut source, lifecycle) = connector.connect(handshake("wss://x/ws")).await.unwrap();
        assert_eq!(lifecycle.snapshot().epoch, 0);
        assert!(source.next().await.is_none());
    }

    #[tokio::test]
    async fn taken_lifecycle_sender_drives_transitions() {
        let connector = MockWsConnector::new();
        let (_sink, _source, mut lifecycle) =
            connector.connect(handshake("wss://x/ws")).await.unwrap();

        let tx = connector.take_lifecycle_sender(0).unwrap();
        assert!(connector.take_lifecycle_sender(0).is_none()); // taken once

        tx.send(LifecycleSnapshot {
            phase: ConnState::Stale,
            ..LifecycleSnapshot::connected(0)
        });
        assert!(lifecycle.changed().await);
        assert_eq!(lifecycle.snapshot().phase, ConnState::Stale);
    }
}
```

Run: `just check` — Expected: FAIL — `cannot find type MockWsConnector`.

- [ ] **Step 5: Implement `MockWsConnector`**

Prepend to `connector.rs`:

```rust
//! A scriptable `WsConnector` leaf: each `connect` consumes the next script
//! (a frame sequence, or a failure), records the handshake, and exposes what
//! every connection sent, whether it closed, and its `LifecycleSender` so a
//! test can drive lifecycle transitions (ADR-0033 §9's "scripted frames +
//! injectable disconnects and `ErrorKind`s").

use crate::sink::ConnectionRecord;
use crate::{MockSink, MockSource, lock};
use oath_adapter_net_ws_api::{
    Frame, Lifecycle, LifecycleSender, LifecycleSnapshot, WsConnector, WsError,
};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
enum Script {
    /// The next connect succeeds; its source yields these items, then ends.
    Yield(VecDeque<Result<Frame, WsError>>),
    /// The next connect fails with this error.
    Fail(WsError),
}

#[derive(Debug, Default)]
struct State {
    scripts: VecDeque<Script>,
    handshakes: Vec<http::Request<()>>,
    records: Vec<Arc<Mutex<ConnectionRecord>>>,
}

/// A scriptable connector leaf that records everything.
///
/// Unscripted `connect`s succeed with an immediately-ended source. Successful
/// connections are numbered from 0 (the index used by the accessors), and
/// connection `n`'s lifecycle is seeded `LifecycleSnapshot::connected(n)`.
#[derive(Debug, Clone, Default)]
pub struct MockWsConnector {
    state: Arc<Mutex<State>>,
}

impl MockWsConnector {
    /// A connector with no scripts queued.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a successful connection whose source yields `items` in order,
    /// then ends. Inject a mid-stream disconnect by scripting an `Err` item.
    pub fn script_connection(&self, items: impl IntoIterator<Item = Result<Frame, WsError>>) {
        lock(&self.state).scripts.push_back(Script::Yield(items.into_iter().collect()));
    }

    /// Enqueue a `connect` failure.
    pub fn script_connect_error(&self, error: WsError) {
        lock(&self.state).scripts.push_back(Script::Fail(error));
    }

    /// How many times `connect` was called (including failed attempts).
    #[must_use]
    pub fn connect_count(&self) -> usize {
        lock(&self.state).handshakes.len()
    }

    /// The handshake requests seen, in order (including failed attempts).
    #[must_use]
    pub fn recorded_handshakes(&self) -> Vec<http::Request<()>> {
        lock(&self.state).handshakes.clone()
    }

    /// Frames sent through successful connection `connection`'s sink, in
    /// order. Empty for an unknown index.
    #[must_use]
    pub fn sent_frames(&self, connection: usize) -> Vec<Frame> {
        lock(&self.state)
            .records
            .get(connection)
            .map(|record| lock(record).sent.clone())
            .unwrap_or_default()
    }

    /// Whether successful connection `connection`'s sink was closed.
    #[must_use]
    pub fn close_called(&self, connection: usize) -> bool {
        lock(&self.state)
            .records
            .get(connection)
            .is_some_and(|record| lock(record).closed)
    }

    /// Take successful connection `connection`'s lifecycle write side, to
    /// drive transitions from the test. `None` for an unknown index or if
    /// already taken.
    pub fn take_lifecycle_sender(&self, connection: usize) -> Option<LifecycleSender> {
        lock(&self.state)
            .records
            .get(connection)
            .and_then(|record| lock(record).lifecycle.take())
    }
}

impl WsConnector for MockWsConnector {
    type Sink = MockSink;
    type Source = MockSource;

    fn connect(
        &self,
        handshake: http::Request<()>,
    ) -> impl Future<Output = Result<(MockSink, MockSource, Lifecycle), WsError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut state = lock(&state);
            state.handshakes.push(handshake);
            let items = match state.scripts.pop_front() {
                Some(Script::Fail(error)) => return Err(error),
                Some(Script::Yield(items)) => items,
                None => VecDeque::new(),
            };
            let epoch = u64::try_from(state.records.len()).unwrap_or(u64::MAX);
            let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(epoch));
            let record = Arc::new(Mutex::new(ConnectionRecord {
                lifecycle: Some(tx),
                ..ConnectionRecord::default()
            }));
            state.records.push(Arc::clone(&record));
            Ok((MockSink::new(record), MockSource::new(items), lifecycle))
        }
    }
}
```

(Note: the async block holds the `MutexGuard` but never `.await`s while holding it, so the future stays `Send`. `MockSink::new` and `ConnectionRecord` are `pub(crate)` in `sink.rs` — the connector constructs them; only the connector's accessors expose their contents.)

Run: `just check && cargo test -p oath-adapter-net-ws-mock && just lint`
Expected: PASS.

- [ ] **Step 6: README graph + CHANGELOG**

`README.md` — in the crate table, after the `oath-adapter-net-http-api` row, add:

```markdown
| `oath-adapter-net-ws-api` | WebSocket transport contract (`Frame`, `WsSink`/`WsSource`, `Lifecycle`, `WsConnector`, …) over the `oath-adapter-net-api` kernel |
```

In the mermaid graph, after the `nethttpapi` line, add:

```text
    netwsapi[oath-adapter-net-ws-api] --> netapi
```

(The dev-only mock crates stay out of the README, as `net-http-mock` does.)

`CHANGELOG.md` — add to `[Unreleased] → Added`:

```markdown
- `oath-adapter-net-ws-api` WebSocket contract (ADR-0032/0033) — `Frame`/`CloseFrame`
  (RFC 6455 frame vocabulary), `WsError` (one concrete transport error with
  `HasErrorKind`), the split owned halves (`WsSink` one-shot RPITIT send half with
  terminal `close(self)`; `WsSource` blanket `Stream` recv half), the epoch-stamped
  lifecycle watch channel (`ConnState`, `LifecycleSnapshot`, `Lifecycle`/
  `LifecycleSender` over runtime-neutral `async-watch`), and the `WsConnector` leaf
  seam. New `oath-adapter-net-ws-mock` test harness (`MockWsConnector`, `MockSink`,
  `MockSource`).
```

- [ ] **Step 7: Full gate**

Run: `just ci`
Expected: green (fmt, fmt-toml, typos, lint, check, test, deny, doc, machete, gitleaks, actionlint, shellcheck).

- [ ] **Step 8: Commit**

```bash
git add crates/adapter/net/ws/mock Cargo.toml README.md CHANGELOG.md
git commit -m "feat(net): net-ws-mock harness — MockWsConnector, MockSink, MockSource"
```

---

## Self-Review

**Spec coverage (ADR-0032 Decision §§1–8 + ADR-0033 §5 amendments):**
- §1 untyped frame channel / grammar-blindness — `Frame` payloads are raw `Bytes`; no venue types anywhere. Task 1. ✅
- §2 asymmetric split — `WsSource` = `Stream` (Task 3), `WsSink` = RPITIT `send` + `close(self)` (Task 3), `connect` triple with `&self` (Task 5). ✅
- §3 `Frame` enum, five variants, data/control distinction — Task 1 (`is_data`/`is_control` feed the ADR-0033 buffer/heartbeat layers). ✅
- §4 lifecycle as separate epoch-stamped channel, `ConnState` incl. `Unrecoverable`, watch-of-`LifecycleSnapshot` with monotonic `epoch`/`attempts`/`total_lagged` (ADR-0033 §5 resolution) — Task 4, including the coalescing/epoch-delta test and the never-blocks producer. ✅
- §5 recovery split, §6 buffer *mechanism*, §7 tungstenite leaf, §8 `AuthSource` — deliberately deferred (adapter/resilience/leaf slices); recorded in the header's deferred list. ✅ (The §6 *guarantee* leaves this slice its hooks: `total_lagged` field + `is_control` bypass predicate.)
- ADR-0032 Consequences "new crates" — `net-ws-api` built here with the runtime-free dep subset it uses; `net-ws-tungstenite` deferred. `WsError: HasErrorKind` once — Task 2. README graph updated (Task 6) since the crate now exists. ✅
- ADR-0033 §9 mock — `MockWsConnector` (scripted frames + injectable disconnects/`ErrorKind`s) — Task 6; `MockTimer`/`MockSpawn` deferred with the layers that need them. ✅

**Placeholder scan:** none — every step has complete code and exact commands.

**Type consistency:** `Frame`/`CloseFrame` (Task 1) used identically in Tasks 2–6; `WsError` constructors (`connection`/`auth`/`other`) match all later uses; `LifecycleSnapshot::connected(epoch)` and `Lifecycle::channel`/`snapshot`/`changed` and `LifecycleSender::send` match Tasks 4–6; `WsConnector::{Sink, Source, connect}` (Task 5) matches the mock impl (Task 6); `MockSink::new(Arc<Mutex<ConnectionRecord>>)` and `MockSource::new(VecDeque<…>)` consistent between sink.rs/source.rs tests and connector.rs.

**Known risks to watch during implementation:**
- `async-watch` 0.3 exact API (`channel`/`borrow`/`changed(&mut)`/`send(&self)`/`Receiver: Clone`) — mirrors `tokio::sync::watch`; verify at Task 4 step 3 and adapt wrapper internals only. Must clear `just deny` (license/advisory).
- `http::Request<()>: Clone` — relied on by `recorded_handshakes`; the landed `MockClient::recorded_requests` already clones `http::Request<Bytes>` under the same `http` version, so this holds.
- Task 6 step ordering: `lib.rs` declares all three modules up front; if `just check` complains mid-task about the not-yet-created `connector.rs`, create it with its placeholder `//!` line first.
