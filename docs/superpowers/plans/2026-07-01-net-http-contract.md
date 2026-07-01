# net-http HTTP Contract (Slice 0, PR 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `oath-adapter-net-http-api` its HTTP data-plane contracts — `HttpError`, `HttpClient`, `ResponseBody`, `BufferMode` — and ship the standalone `oath-adapter-net-http-mock` test harness, so later slices (auth/body/rate layers, then assembly) have a typed, mockable HTTP surface to build on.

**Architecture:** The stack is pure transport — bytes in, bytes out. `HttpError` is one concrete error for **transport/middleware failures only** (HTTP 4xx/5xx statuses are NOT error-ified; they flow through as `Ok(Response)` with body intact for the adapter to classify). `HttpClient` is a blanket-impl'd `Service` sub-trait so any backend is `HttpClient` for free. `ResponseBody<B>` is a `pin-project-lite` enum (buffered `Full<Bytes>` xor streaming `B`) that forwards all three `Body` methods. The mock crate is a self-contained harness (`MockClient`, `MockBody`, `MockTimer`) consumed by downstream slices via `[dev-dependencies]`.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, `http`/`http-body`/`http-body-util`/`bytes`/`pin-project-lite`/`thiserror`. **No** `tokio`/`hyper`/`reqwest`/`serde` — `net-http-api` stays runtime-free.

**Source spec:** [docs/superpowers/specs/2026-06-30-net-http-construction-surface-design.md](../specs/2026-06-30-net-http-construction-surface-design.md). This is **Slice 0, PR 2**; it builds on PR 1 (#57, `feat/net-http-api-repartition`) and follows the PR 2 roadmap in [2026-06-30-net-http-foundation.md](2026-06-30-net-http-foundation.md).

**Depends on PR 1 having merged** (or a branch stacked on it): PR 2 consumes `oath_adapter_net_http_api::Service`, `oath_adapter_net_api::{ErrorKind, HasErrorKind, Timer}`.

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` (`unsafe_code = "deny"`). Body impls use `pin-project-lite`, never manual `unsafe`.
- **No `unwrap`/`expect`/indexing/panic in non-test code** — return `Result` / recover (`Mutex` poison via `unwrap_or_else(std::sync::PoisonError::into_inner)`). Test code is exempt for `unwrap`/`expect`/indexing only.
- **`just lint` runs `clippy --all-targets -- -D warnings`, which promotes `pedantic`/`nursery` (warn-level) to errors** — so all code, **including tests**, must be pedantic-clean: no `as` casts that trip `cast_possible_truncation` (use `u64::try_from(x).unwrap_or(u64::MAX)` or pick a `u64` field), add `#[must_use]` where clippy asks, document all public items (`missing_docs`), derive `Debug` (`missing_debug_implementations`), no unreachable `pub`.
- **`net-http-api` charter:** no `tokio`/`hyper`/`reqwest`/`serde`; free of any async runtime. Adds only `http`/`bytes`/`http-body`/`http-body-util`/`pin-project-lite`/`thiserror` (+ `oath-adapter-net-api`) as tasks use them.
- **`HttpError` is model A:** transport/middleware failures only. HTTP error statuses are never converted to `HttpError`.
- **Deps** via `[workspace.dependencies]` (explicit `version` for internal crates). Add a dep in the task that first *uses* it (keeps `cargo-machete` green).
- **DoD per PR:** `just ci` green. Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree under `.claude/worktrees/<slug>` → one PR (`Closes #<issue>`).

---

## File Structure

- `crates/adapter/net/http/api/src/error.rs` — **new.** `HttpError`, `BoxError`, `HasErrorKind` impl.
- `crates/adapter/net/http/api/src/client.rs` — **new.** `HttpClient` trait + blanket impl.
- `crates/adapter/net/http/api/src/body.rs` — **new.** `ResponseBody<B>`, `BufferMode`.
- `crates/adapter/net/http/api/src/lib.rs` — **modify.** Add `mod`/`pub use` for the above.
- `crates/adapter/net/http/api/Cargo.toml` — **modify.** Add deps as used.
- `crates/adapter/net/http/mock/{Cargo.toml,src/lib.rs,src/body.rs,src/client.rs,src/timer.rs}` — **new crate** `oath-adapter-net-http-mock`.
- `Cargo.toml` (workspace) — **modify.** Add `http-body-util`/`pin-project-lite` to `[workspace.dependencies]`; register the mock crate as a member + dep entry.
- `CHANGELOG.md` — **modify.**

Each PR-2 task is one commit; the four tasks together are one PR/issue.

---

## Task 2.1: `HttpError` + `HasErrorKind`

**Files:**
- Create: `crates/adapter/net/http/api/src/error.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`, `crates/adapter/net/http/api/Cargo.toml`

**Interfaces:**
- Consumes: `oath_adapter_net_api::{ErrorKind, HasErrorKind}` (kernel).
- Produces: `oath_adapter_net_http_api::{HttpError, BoxError}`. `HttpError::auth(impl Into<String>) -> HttpError`, `HttpError::connection(impl Into<BoxError>)`, `HttpError::other(impl Into<BoxError>)`. `impl HasErrorKind for HttpError`.

- [ ] **Step 1: Add deps**

In `crates/adapter/net/http/api/Cargo.toml`, add a `[dependencies]` section:

```toml
[dependencies]
oath-adapter-net-api = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/http/api/src/error.rs` with only the test, and add `pub mod error;` to `lib.rs`:

```rust
//! The single concrete error type for the HTTP stack — placeholder; filled in step 4.

#[cfg(test)]
mod tests {
    use super::HttpError;
    use oath_adapter_net_api::{ErrorKind, HasErrorKind};

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(HttpError::Timeout.kind(), ErrorKind::Timeout);
        assert_eq!(HttpError::connection("reset").kind(), ErrorKind::Connection);
        assert_eq!(HttpError::Throttled.kind(), ErrorKind::Throttled);
        assert_eq!(HttpError::auth("expired").kind(), ErrorKind::Auth);
        assert_eq!(HttpError::other("boom").kind(), ErrorKind::Unknown);
    }

    #[test]
    fn auth_carries_message() {
        assert_eq!(HttpError::auth("no token").to_string(), "authorization failed: no token");
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HttpError>();
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type HttpError`.

- [ ] **Step 4: Implement `HttpError`**

Prepend to `error.rs` (above the test), replacing the placeholder `//!` line:

```rust
//! The single concrete error type across the HTTP stack — transport and
//! middleware failures only. HTTP 4xx/5xx *statuses* are NOT errors here: they
//! flow through as `Ok(http::Response)` with the body intact for the adapter to
//! classify (ADR-0030 §5). Retry/CircuitBreaker peek `Response::status()` for
//! their resilience decisions.

use oath_adapter_net_api::{ErrorKind, HasErrorKind};

/// A boxed error source, preserving backend detail for logs without leaking the
/// concrete type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The single `Service::Error` (and every `Body::Error`) of the HTTP stack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpError {
    /// The request did not complete within its timeout.
    #[error("request timed out")]
    Timeout,
    /// A connection-level failure (DNS, TCP, TLS, backend transport).
    #[error("connection failure")]
    Connection(#[source] BoxError),
    /// A pacing wait exceeded `max_wait` — the request was not sent.
    #[error("throttled: pacing wait exceeded max_wait")]
    Throttled,
    /// Credential stamping or refresh failed.
    #[error("authorization failed: {0}")]
    Auth(String),
    /// A backend error that does not fit another variant.
    #[error("network error")]
    Other(#[source] BoxError),
}

impl HttpError {
    /// Construct an [`HttpError::Auth`] from a message. `AuthSource` impls use this.
    #[must_use]
    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    /// Construct an [`HttpError::Connection`] from a source error.
    #[must_use]
    pub fn connection(source: impl Into<BoxError>) -> Self {
        Self::Connection(source.into())
    }

    /// Construct an [`HttpError::Other`] from a source error.
    #[must_use]
    pub fn other(source: impl Into<BoxError>) -> Self {
        Self::Other(source.into())
    }
}

impl HasErrorKind for HttpError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Timeout => ErrorKind::Timeout,
            Self::Connection(_) => ErrorKind::Connection,
            Self::Throttled => ErrorKind::Throttled,
            Self::Auth(_) => ErrorKind::Auth,
            Self::Other(_) => ErrorKind::Unknown,
        }
    }
}
```

Add to `lib.rs`: `pub mod error;` and `pub use error::{BoxError, HttpError};`, plus an `//! - [`error`] — `HttpError`` line in the module-list doc.

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-http-api error && just lint`
Expected: PASS, warning-free.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/http/api/src/error.rs crates/adapter/net/http/api/src/lib.rs crates/adapter/net/http/api/Cargo.toml
git commit -m "feat(net): HttpError — one concrete transport/middleware error, HasErrorKind"
```

---

## Task 2.2: `HttpClient` blanket-impl'd `Service` sub-trait

**Files:**
- Create: `crates/adapter/net/http/api/src/client.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`, `crates/adapter/net/http/api/Cargo.toml`

**Interfaces:**
- Consumes: `crate::{Service, HttpError}`; `http`, `bytes`, `http-body`.
- Produces: `oath_adapter_net_http_api::HttpClient` — a trait with `type Body: http_body::Body<Data = Bytes, Error = HttpError>` and `fn send(&self, http::Request<Bytes>) -> impl Future<Output = Result<http::Response<Self::Body>, HttpError>> + Send`; blanket `impl<S, B> HttpClient for S`.

- [ ] **Step 1: Add deps**

Append to `crates/adapter/net/http/api/Cargo.toml` `[dependencies]`:

```toml
http = { workspace = true }
bytes = { workspace = true }
http-body = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/http/api/src/client.rs` with the test only; add `pub mod client;` to `lib.rs`:

```rust
//! The `HttpClient` dependency-inversion seam — placeholder; filled in step 4.

#[cfg(test)]
mod tests {
    use super::HttpClient;
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // Minimal body whose error is `HttpError` (stock `Full`/`Empty` are `Infallible`).
    struct EmptyBody;
    impl Body for EmptyBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(self: Pin<&mut Self>, _: &mut Context<'_>)
            -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(None)
        }
        fn is_end_stream(&self) -> bool { true }
        fn size_hint(&self) -> SizeHint { SizeHint::with_exact(0) }
    }

    #[derive(Clone)]
    struct Leaf;
    impl Service<http::Request<Bytes>> for Leaf {
        type Response = http::Response<EmptyBody>;
        type Error = HttpError;
        fn call(&self, _req: http::Request<Bytes>)
            -> impl std::future::Future<Output = Result<Self::Response, HttpError>> + Send {
            async { Ok(http::Response::new(EmptyBody)) }
        }
    }

    #[test]
    fn any_matching_service_is_httpclient() {
        fn assert_http_client<C: HttpClient>(_: &C) {}
        assert_http_client(&Leaf); // blanket impl applies
    }

    #[tokio::test]
    async fn send_is_sugar_over_call() {
        let resp = HttpClient::send(&Leaf, http::Request::new(Bytes::new())).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
    }
}
```

Add `tokio` as a **dev-dependency** to `crates/adapter/net/http/api/Cargo.toml` (test-only executor; does not touch the runtime-free production graph):

```toml
[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find trait HttpClient`.

- [ ] **Step 4: Implement `HttpClient`**

Prepend to `client.rs`:

```rust
//! The named dependency-inversion seam adapters code against. It *is* a
//! [`Service`], so the `Layer` machinery composes it; a backend implements
//! `Service` once and is `HttpClient` for free (ADR-0030 §6). Per ADR-0029 §5 it
//! is a compile-time `impl HttpClient` seam — never `dyn`.

use crate::{HttpError, Service};
use bytes::Bytes;
use std::future::Future;

/// A composed HTTP client: a [`Service`] from `http::Request<Bytes>` to
/// `http::Response<Self::Body>` with `Error = HttpError`.
pub trait HttpClient:
    Service<http::Request<Bytes>, Response = http::Response<Self::Body>, Error = HttpError>
{
    /// The response body type (generic, for zero-alloc flow-through).
    type Body: http_body::Body<Data = Bytes, Error = HttpError>;

    /// Send a request — sugar over [`Service::call`].
    fn send(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<http::Response<Self::Body>, HttpError>> + Send {
        self.call(req)
    }
}

impl<S, B> HttpClient for S
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>,
    B: http_body::Body<Data = Bytes, Error = HttpError>,
{
    type Body = B;
}
```

Add to `lib.rs`: `pub mod client;`, `pub use client::HttpClient;`, module-doc line.

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-http-api client && just lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/http/api/src/client.rs crates/adapter/net/http/api/src/lib.rs crates/adapter/net/http/api/Cargo.toml
git commit -m "feat(net): HttpClient — blanket-impl'd Service sub-trait with send sugar"
```

---

## Task 2.3: `ResponseBody<B>` + `BufferMode`

**Files:**
- Create: `crates/adapter/net/http/api/src/body.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`, `crates/adapter/net/http/api/Cargo.toml`, root `Cargo.toml`

**Interfaces:**
- Consumes: `crate::HttpError`; `http-body`, `http-body-util` (`Full`), `bytes`, `pin-project-lite`.
- Produces: `oath_adapter_net_http_api::ResponseBody<B>` with `ResponseBody::buffered(Bytes) -> Self` and `ResponseBody::streaming(B) -> Self`, and `impl<B: Body<Data=Bytes,Error=HttpError>> Body for ResponseBody<B>` forwarding `poll_frame`/`is_end_stream`/`size_hint`. `oath_adapter_net_http_api::BufferMode` (`Buffer`/`Stream`, `Copy`).

- [ ] **Step 1: Add deps**

In the root `Cargo.toml` `[workspace.dependencies]`, add:

```toml
http-body-util = "0.1"
pin-project-lite = "0.2"
```

In `crates/adapter/net/http/api/Cargo.toml` `[dependencies]`, add:

```toml
http-body-util = { workspace = true }
pin-project-lite = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `crates/adapter/net/http/api/src/body.rs` with the test only; add `pub mod body;` to `lib.rs`:

```rust
//! The canonical response body + buffer-mode directive — placeholder; step 4.

#[cfg(test)]
mod tests {
    use super::{BufferMode, ResponseBody};
    use crate::HttpError;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // Inner body with a known, non-default size_hint / is_end_stream, so the
    // parity assertion is meaningful.
    struct Stub {
        remaining: u64,
    }
    impl Body for Stub {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(self: Pin<&mut Self>, _: &mut Context<'_>)
            -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(None)
        }
        fn is_end_stream(&self) -> bool { self.remaining == 0 }
        fn size_hint(&self) -> SizeHint { SizeHint::with_exact(self.remaining) }
    }

    #[test]
    fn streaming_forwards_size_hint_and_is_end_stream() {
        let reference = Stub { remaining: 42 };
        let ref_hint = reference.size_hint().exact();
        let ref_end = reference.is_end_stream();
        let wrapped = ResponseBody::streaming(Stub { remaining: 42 });
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently None/unbounded
        assert_eq!(wrapped.is_end_stream(), ref_end);
    }

    #[test]
    fn buffered_reports_exact_length() {
        let body: ResponseBody<Stub> = ResponseBody::buffered(Bytes::from_static(b"hello"));
        assert_eq!(body.size_hint().exact(), Some(5));
    }

    #[test]
    fn buffer_mode_is_copy() {
        let m = BufferMode::Buffer;
        let n = m; // Copy
        assert_eq!(m, n);
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type ResponseBody` / `BufferMode`.

- [ ] **Step 4: Implement `ResponseBody` + `BufferMode`**

Prepend to `body.rs`:

```rust
//! The canonical HTTP response body and the per-request buffer/stream directive.

use crate::HttpError;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::Full;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Per-request directive: buffer the response body inside the retry boundary, or
/// return it streaming at headers (ADR-0030 §4). `Copy` so it survives the
/// request clone `Retry` makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMode {
    /// Collect the body to `Bytes` before returning (full retry coverage).
    Buffer,
    /// Return the live body at headers (adapter owns mid-stream recovery).
    Stream,
}

pin_project_lite::pin_project! {
    /// The canonical response body: one buffered frame *xor* a live streaming
    /// body, behind one stable type so adapters never name the buffer-vs-stream
    /// machinery. Forwards all three `Body` methods to the active arm — a
    /// wrapper that silently reported the default `size_hint`/`is_end_stream`
    /// would make a caller's `.collect()` pre-size and any max-size guard wrong.
    #[project = ResponseBodyProj]
    pub enum ResponseBody<B> {
        /// A fully-collected body (single frame).
        Buffered { #[pin] body: Full<Bytes> },
        /// A live streaming backend body.
        Streaming { #[pin] body: B },
    }
}

impl<B> ResponseBody<B> {
    /// Wrap already-collected bytes as a one-frame buffered body.
    #[must_use]
    pub fn buffered(bytes: Bytes) -> Self {
        Self::Buffered { body: Full::new(bytes) }
    }

    /// Wrap a live streaming backend body.
    pub fn streaming(body: B) -> Self {
        Self::Streaming { body }
    }
}

impl<B> Body for ResponseBody<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        match self.project() {
            // `Full`'s error is `Infallible`; the `Err` arm is unreachable.
            ResponseBodyProj::Buffered { body } => match body.poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(never))) => match never {},
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            ResponseBodyProj::Streaming { body } => body.poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Buffered { body } => body.is_end_stream(),
            Self::Streaming { body } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Buffered { body } => body.size_hint(),
            Self::Streaming { body } => body.size_hint(),
        }
    }
}
```

Add to `lib.rs`: `pub mod body;`, `pub use body::{BufferMode, ResponseBody};`, module-doc line.

- [ ] **Step 5: Run tests**

Run: `just check && cargo test -p oath-adapter-net-http-api body && just lint`
Expected: PASS. (If `just machete` later flags `http-body-util`/`pin-project-lite` as unused, they are used here — re-run after this task lands.)

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/http/api/src/body.rs crates/adapter/net/http/api/src/lib.rs crates/adapter/net/http/api/Cargo.toml Cargo.toml
git commit -m "feat(net): ResponseBody (buffer-xor-stream, forwards Body metadata) + BufferMode"
```

---

## Task 2.4: `oath-adapter-net-http-mock` harness

**Files:**
- Create: `crates/adapter/net/http/mock/Cargo.toml`, `.../src/lib.rs`, `.../src/body.rs`, `.../src/client.rs`, `.../src/timer.rs`
- Modify: root `Cargo.toml` (member + dep entry)

**Interfaces:**
- Consumes: `oath_adapter_net_http_api::{Service, HttpError}`; `oath_adapter_net_api::Timer`; `http`, `bytes`, `http-body`.
- Produces: `oath_adapter_net_http_mock::{MockBody, MockClient, MockTimer}`. `MockBody::new(frames)`, `MockBody::empty()`. `MockClient::ok(Bytes)`, `MockClient::new(StatusCode, frames)`, `MockClient::recorded_requests() -> Vec<http::Request<Bytes>>`. `MockTimer::new() -> Self`, `MockTimer::advance(Duration)`.

*(Standalone harness — `net-http-api` does NOT depend on it, so there is no dev-dep cycle. Downstream slices add it under their own `[dev-dependencies]`.)*

- [ ] **Step 1: Register the crate**

Root `Cargo.toml`: add `"crates/adapter/net/http/mock",` to `members`, and to `[workspace.dependencies]`:

```toml
oath-adapter-net-http-mock = { path = "crates/adapter/net/http/mock", version = "0.1.0" }
```

Create `crates/adapter/net/http/mock/Cargo.toml`:

```toml
[package]
name = "oath-adapter-net-http-mock"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
oath-adapter-net-api = { workspace = true }
oath-adapter-net-http-api = { workspace = true }
http = { workspace = true }
bytes = { workspace = true }
http-body = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

Create `crates/adapter/net/http/mock/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: `MockBody` — write the failing test**

Create `crates/adapter/net/http/mock/src/body.rs`:

```rust
//! A response body that yields pre-set frames — placeholder; step 3.

#[cfg(test)]
mod tests {
    use super::MockBody;
    use bytes::Bytes;
    use http_body::Body;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn yields_frames_then_ends_and_reports_exact_size() {
        let body = MockBody::new([Bytes::from_static(b"ab"), Bytes::from_static(b"cde")]);
        assert_eq!(body.size_hint().exact(), Some(5));
        assert!(!body.is_end_stream());
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected, Bytes::from_static(b"abcde"));
    }
}
```

Add `http-body-util` as a **dev-dependency** of the mock crate (for `BodyExt::collect` in tests) — append to its `Cargo.toml` `[dev-dependencies]`: `http-body-util = { workspace = true }`.

- [ ] **Step 3: Implement `MockBody`**

Prepend to `body.rs`:

```rust
//! A response body that yields pre-set data frames, with a controllable
//! `size_hint`/`is_end_stream` for exercising body-metadata forwarding.

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use oath_adapter_net_http_api::HttpError;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A body that yields its configured frames in order, then ends.
#[derive(Debug, Default)]
pub struct MockBody {
    frames: VecDeque<Bytes>,
}

impl MockBody {
    /// A body yielding `frames` in order.
    #[must_use]
    pub fn new(frames: impl IntoIterator<Item = Bytes>) -> Self {
        Self { frames: frames.into_iter().collect() }
    }

    /// An immediately-ended body.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

impl Body for MockBody {
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        // `MockBody` holds no pinned fields, so `get_mut` is sound (auto-`Unpin`).
        let this = self.get_mut();
        match this.frames.pop_front() {
            Some(data) => Poll::Ready(Some(Ok(Frame::data(data)))),
            None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        let total: u64 = self
            .frames
            .iter()
            .map(|f| u64::try_from(f.len()).unwrap_or(u64::MAX))
            .sum();
        SizeHint::with_exact(total)
    }
}
```

Run: `just check && cargo test -p oath-adapter-net-http-mock body && just lint` — Expected: PASS.

- [ ] **Step 4: `MockTimer` — write the failing test**

Create `crates/adapter/net/http/mock/src/timer.rs`:

```rust
//! A virtual, controllable clock — placeholder; step 5.

#[cfg(test)]
mod tests {
    use super::MockTimer;
    use oath_adapter_net_api::Timer;
    use std::time::Duration;

    #[tokio::test]
    async fn advance_moves_now_and_wakes_sleepers() {
        let timer = MockTimer::new();
        let start = timer.now();
        let sleep = timer.sleep(Duration::from_secs(10));
        let advancer = timer.clone();
        // Wake the sleeper by advancing past its deadline on another task.
        let handle = tokio::spawn(async move { sleep.await });
        tokio::task::yield_now().await;
        advancer.advance(Duration::from_secs(10));
        handle.await.unwrap();
        assert_eq!(timer.now().duration_since(start), Duration::from_secs(10));
    }
}
```

- [ ] **Step 5: Implement `MockTimer`**

Prepend to `timer.rs`:

```rust
//! A virtual clock for deterministically driving timing layers in tests.
//!
//! `std::time::Instant` has no value constructor, so `MockTimer` anchors to a
//! real `Instant::now()` at construction and advances via a stored offset
//! (behind interior mutability, since `Timer::now` takes `&self`). `sleep`
//! registers a waker released by `advance` — a no-op `sleep` would make
//! elapsed-time-dependent tests vacuous. Cf. `governor::clock::FakeRelativeClock`.

use oath_adapter_net_api::Timer;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct State {
    now: Instant,
    waiters: Vec<(Instant, Waker)>,
}

/// A cloneable virtual clock. Clones share one timeline.
#[derive(Debug, Clone)]
pub struct MockTimer {
    state: Arc<Mutex<State>>,
}

impl MockTimer {
    /// A clock anchored at the current real instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State { now: Instant::now(), waiters: Vec::new() })),
        }
    }

    /// Advance virtual time by `dur`, waking every sleeper now due.
    pub fn advance(&self, dur: Duration) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.now += dur;
        let now = state.now;
        let mut due = Vec::new();
        state.waiters.retain(|(deadline, waker)| {
            if *deadline <= now {
                due.push(waker.clone());
                false
            } else {
                true
            }
        });
        drop(state); // release before waking, so a woken poll can re-lock
        for waker in due {
            waker.wake();
        }
    }
}

impl Default for MockTimer {
    fn default() -> Self {
        Self::new()
    }
}

/// The future returned by [`MockTimer::sleep`].
#[derive(Debug)]
pub struct Sleep {
    state: Arc<Mutex<State>>,
    deadline: Instant,
}

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.now >= self.deadline {
            Poll::Ready(())
        } else {
            state.waiters.push((self.deadline, cx.waker().clone()));
            Poll::Pending
        }
    }
}

impl Timer for MockTimer {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send {
        let deadline = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.now + dur
        };
        Sleep { state: Arc::clone(&self.state), deadline }
    }

    fn now(&self) -> Instant {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).now
    }
}
```

Run: `just check && cargo test -p oath-adapter-net-http-mock timer && just lint` — Expected: PASS.

- [ ] **Step 6: `MockClient` — write the failing test**

Create `crates/adapter/net/http/mock/src/client.rs`:

```rust
//! A canned-response client leaf — placeholder; step 7.

#[cfg(test)]
mod tests {
    use super::MockClient;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::HttpClient;

    #[tokio::test]
    async fn returns_canned_body_and_records_requests() {
        let client = MockClient::ok(Bytes::from_static(b"pong"));
        let mut req = http::Request::new(Bytes::from_static(b"ping"));
        *req.uri_mut() = "/tickle".parse().unwrap();
        let resp = client.send(req).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"pong"));
        let recorded = client.recorded_requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].uri(), "/tickle");
    }
}
```

- [ ] **Step 7: Implement `MockClient`**

Prepend to `client.rs`:

```rust
//! A canned-response `Service` leaf that records the requests it receives.

use crate::MockBody;
use bytes::Bytes;
use oath_adapter_net_http_api::{HttpError, Service};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// A leaf client that returns a fixed status + body and records every request.
#[derive(Debug, Clone)]
pub struct MockClient {
    status: http::StatusCode,
    frames: Vec<Bytes>,
    requests: Arc<Mutex<Vec<http::Request<Bytes>>>>,
}

impl MockClient {
    /// A client returning `status` with a body of `frames`.
    #[must_use]
    pub fn new(status: http::StatusCode, frames: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status,
            frames: frames.into_iter().collect(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A `200 OK` client whose body is `body`.
    #[must_use]
    pub fn ok(body: Bytes) -> Self {
        Self::new(http::StatusCode::OK, [body])
    }

    /// The requests this client has received, in order.
    #[must_use]
    pub fn recorded_requests(&self) -> Vec<http::Request<Bytes>> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Service<http::Request<Bytes>> for MockClient {
    type Response = http::Response<MockBody>;
    type Error = HttpError;

    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        let requests = Arc::clone(&self.requests);
        let status = self.status;
        let frames = self.frames.clone();
        async move {
            requests.lock().unwrap_or_else(PoisonError::into_inner).push(req);
            let mut resp = http::Response::new(MockBody::new(frames));
            *resp.status_mut() = status;
            Ok(resp)
        }
    }
}
```

Run: `just check && cargo test -p oath-adapter-net-http-mock client && just lint` — Expected: PASS.

- [ ] **Step 8: CHANGELOG + full gate + commit**

Add to `CHANGELOG.md` `[Unreleased] → Added`:

```markdown
- `oath-adapter-net-http-api` HTTP contract — `HttpError` (one concrete
  transport/middleware error; HTTP statuses pass through as `Ok(Response)`),
  `HttpClient` (blanket-impl'd `Service` sub-trait), `ResponseBody` (buffer-xor-
  stream, forwarding `Body` metadata), and `BufferMode`. New `oath-adapter-net-
  http-mock` test harness (`MockClient`, `MockBody`, `MockTimer`).
```

Run: `just ci` — Expected: green.

```bash
git add crates/adapter/net/http/mock Cargo.toml CHANGELOG.md
git commit -m "feat(net): net-http-mock harness — MockClient, MockBody, MockTimer"
```

---

## Self-Review

**Spec coverage (PR 2 roadmap in the foundation plan + construction-surface spec):**
- `HttpError` model A (transport/middleware only; statuses pass through) — Task 2.1. ✅
- `HttpClient` blanket impl + `send` — Task 2.2. ✅
- `ResponseBody` forwarding all three `Body` methods + `BufferMode` — Task 2.3 (the spec's transparency fix is the `is_end_stream`/`size_hint` match arms + the parity test). ✅
- `net-http-mock` (`MockClient`, `MockBody`, `MockTimer` with observable `sleep`/`advance`) — Task 2.4. ✅
- No dev-dep cycle (net-http-api uses inline doubles) — the roadmap's stated alternative. ✅
- Deferred (correctly absent): `AuthSource`/`Auth`/`Guarded` (PR 3), `RateKey`/coverage (PR 4), the resilience layers + `stack`/`build` + hyper leaf (later slices), `CircuitOpen` variant (added with the CB layer).

**Placeholder scan:** none — every step carries real code/commands.

**Type consistency:** `HttpError` variants/constructors identical across 2.1 and their uses in 2.2–2.4; `Service`/`HttpClient` signatures match the PR-1 landed `Service` and the 2.2 definition; `MockBody`/`MockClient`/`MockTimer` names match `lib.rs` re-exports and the `Interfaces` blocks. `ResponseBody` arm names (`Buffered`/`Streaming`) consistent between the pin-project enum, the `project`ed `poll_frame`, and the `&self` `is_end_stream`/`size_hint` matches.

**Known risk to watch during impl:** `http_body_util`/`pin_project_lite` exact API surface (`Full::new`, `Frame::data`, `SizeHint::with_exact`, `pin_project!` enum `#[project = …]`) — all stable in the pinned versions; if a signature differs, adjust at the failing-test step. The mock crate's dev-dep on `tokio`/`http-body-util` is test-only and does not touch any production graph.
