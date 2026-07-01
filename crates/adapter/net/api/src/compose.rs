//! Composition machinery: `Layer`, `ServiceBuilder`, `Identity`, `Stack`.
//!
//! These compose **anything** — `Layer<S>` carries no `Service` bound (ADR-0029
//! §3), so the same machinery composes an HTTP `Service` stack today and a WS
//! subscription stack tomorrow.
//!
//! # Ordering invariant
//!
//! The **first** `.layer()` call is permanently the outermost wrapper and
//! therefore the first to handle each request.
//!
//! ```
//! # use oath_adapter_net_api::compose::{Layer, ServiceBuilder};
//! # struct TracingLayer;
//! # struct MetricsLayer;
//! # impl<S> Layer<S> for TracingLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! # impl<S> Layer<S> for MetricsLayer { type Service = S; fn layer(&self, s: S) -> S { s } }
//! // TracingLayer is added first → outermost → wraps everything else.
//! let _svc = ServiceBuilder::new()
//!     .layer(TracingLayer) // outermost
//!     .layer(MetricsLayer) // inner
//!     .service(());        // leaf: any value (a `Service` leaf lives in net-http-api)
//! ```

/// Transform one [`Layer`] into another [`Layer`].
///
/// Typically a struct that holds configuration and owns an inner value. The
/// outer layer's [`Layer::layer`] method wraps the inner value, producing a
/// new value that adds the layer's behaviour.
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
/// # use oath_adapter_net_api::compose::{Identity, ServiceBuilder};
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

    /// Finalize the stack by wrapping a concrete value.
    ///
    /// Consumes the builder and returns the fully composed value.
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
