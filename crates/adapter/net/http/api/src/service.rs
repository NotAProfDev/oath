//! The request/reply connection-shape contract for the HTTP transport.
//!
//! `Service` models **request → one reply** — it fits REST and unary RPC but
//! not WebSocket subscriptions or multicast, so it is a per-transport contract,
//! not a kernel primitive (ADR-0029 §2). It is transport-*neutral* (names no
//! HTTP type); it lives here, the first request/reply transport, until a second
//! one justifies hoisting it into a shared `net-req-reply-api` crate.

use std::future::Future;

/// A single async call: request in, `Result` out.
///
/// `call` takes `&self` (not `&mut self`) — a service is shared, not owned, by
/// its callers. The returned future is **`Send`** (enforced here) so it runs on
/// a multithreaded runtime. The service *value* is expected to be
/// `Send + Sync + 'static` too, but that is enforced at the **composition
/// boundary** (`stack()`'s return bound, ADR-0030), not on this trait — so a
/// service may be non-`Sync` in a context that never shares it. Backpressure is
/// handled inside `call` (e.g. awaiting a permit), not via a separate
/// `poll_ready`.
///
/// Because the `call` future borrows `&self`, it is **not** `'static`-spawnable:
/// to `tokio::spawn` a call, clone the (cheap, `Arc`-backed) service and move the
/// clone in. RPITIT return — no `async-trait`, no `dyn`, no per-call allocation.
pub trait Service<Req> {
    /// The value produced on success.
    type Response;

    /// The error produced on failure.
    type Error;

    /// Drive the request to completion.
    fn call(&self, req: Req) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send;
}
