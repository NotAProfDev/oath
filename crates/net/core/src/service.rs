//! Core composition primitives: `Service`, `Layer`, `ServiceBuilder`, `Identity`, `Stack`.
//!
//! These are the building blocks for the entire network stack. Every middleware
//! concern is expressed as a [`Layer`] that wraps any [`Service`], and
//! [`ServiceBuilder`] composes them at compile time with no virtual dispatch.
//!
//! # Ordering invariant
//!
//! The **first** `.layer()` call is permanently the outermost wrapper and
//! therefore the first to handle each request. Subsequent calls are nested
//! progressively further inward.
//!
//! ```no_run
//! # use oath_net_core::service::{Layer, Service, ServiceBuilder};
//! # use std::future::Future;
//! # struct TracingLayer;
//! # struct MetricsLayer;
//! # struct Transport;
//! # impl<S> Layer<S> for TracingLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! # impl<S> Layer<S> for MetricsLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! # impl Service<()> for Transport {
//! #     type Response = ();
//! #     type Error = ();
//! #     fn call(&self, _: ()) -> impl Future<Output = Result<(), ()>> + Send {
//! #         async { Ok(()) }
//! #     }
//! # }
//! // TracingLayer is added first → it is outermost → handles every request first.
//! let svc = ServiceBuilder::new()
//!     .layer(TracingLayer) // outermost: first to see each request
//!     .layer(MetricsLayer) // innermost of the two wrappers
//!     .service(Transport); // leaf: performs actual I/O
//! ```

use std::future::Future;

/// A single async call: request in, `Result` out.
///
/// Implementations must not require `&mut self` — services are shared across
/// tasks and must therefore be `Send + Sync`. Backpressure is handled inside
/// `call` (e.g. by awaiting a semaphore permit) rather than through a separate
/// `poll_ready`.
///
/// Use RPITIT for the return type — no `async-trait`, no `dyn`, no per-call
/// allocation.
pub trait Service<Req> {
    /// The value produced on success.
    type Response;

    /// The error produced on failure.
    type Error;

    /// Drive the request to completion.
    fn call(&self, req: Req) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send;
}

/// Transform one [`Service`] into another [`Service`].
///
/// Typically a struct that holds configuration and owns an inner service. The
/// outer layer's [`Layer::layer`] method wraps the inner service, producing a
/// new [`Service`] that adds the layer's behaviour.
pub trait Layer<S> {
    /// The wrapped service type produced by this layer.
    type Service;

    /// Wrap `inner` with this layer's behaviour.
    fn layer(&self, inner: S) -> Self::Service;
}

/// Type-safe layer compositor.
///
/// Layers are applied in **declaration order**: the first `.layer()` call is
/// the outermost wrapper and therefore the first to execute on each request.
///
/// ```
/// # use oath_net_core::service::{Identity, ServiceBuilder};
/// let _builder = ServiceBuilder::new(); // starts with Identity (no-op)
/// ```
#[derive(Debug, Clone)]
pub struct ServiceBuilder<L> {
    layer: L,
}

impl Default for ServiceBuilder<Identity> {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceBuilder<Identity> {
    /// Create a new builder with no layers applied.
    #[must_use]
    pub const fn new() -> Self {
        Self { layer: Identity }
    }
}

impl<L> ServiceBuilder<L> {
    /// Add a new layer `New` into this `ServiceBuilder`.
    ///
    /// `New` becomes the new `Inner`; the accumulated `L` remains `Outer` and
    /// therefore executes first on every request. This preserves the invariant
    /// that the **first** `.layer()` call stays permanently outermost.
    #[must_use]
    pub fn layer<New>(self, layer: New) -> ServiceBuilder<Stack<New, L>> {
        ServiceBuilder {
            layer: Stack {
                inner: layer,
                outer: self.layer,
            },
        }
    }

    /// Finalize the stack by wrapping a concrete service.
    ///
    /// Consumes the builder and returns the fully composed `Service` value.
    /// The concrete type is fully resolved at compile time — no boxing, no
    /// `dyn`.
    pub fn service<S>(self, service: S) -> L::Service
    where
        L: Layer<S>,
    {
        self.layer.layer(service)
    }
}

/// The no-op layer — passes the inner service through unchanged.
///
/// `Identity` is the initial state of a fresh [`ServiceBuilder`].
#[derive(Debug, Clone, Copy)]
pub struct Identity;

impl<S> Layer<S> for Identity {
    type Service = S;

    fn layer(&self, inner: S) -> S {
        inner
    }
}

/// Compose two [`Layer`] impls into one.
///
/// When assembling the stack, `Inner.layer(leaf)` is applied first, then
/// `Outer.layer(result)`. `Outer` is therefore the outermost service and the
/// first to handle each request.
///
/// Because [`ServiceBuilder::layer`] produces `Stack<New, L>` with `New` in
/// the `Inner` slot and the accumulated `L` in the `Outer` slot, each new
/// layer is nested *inside* the existing stack — leaving the first `.layer()`
/// call's layer permanently outermost.
#[derive(Debug, Clone, Copy)]
pub struct Stack<Inner, Outer> {
    inner: Inner,
    outer: Outer,
}

impl<S, Inner, Outer> Layer<S> for Stack<Inner, Outer>
where
    Inner: Layer<S>,
    Outer: Layer<Inner::Service>,
{
    type Service = Outer::Service;

    fn layer(&self, service: S) -> Outer::Service {
        // Apply Inner first (closer to the leaf), then wrap with Outer.
        let inner_svc = self.inner.layer(service);
        self.outer.layer(inner_svc)
    }
}
