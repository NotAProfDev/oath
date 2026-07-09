//! Composition machinery: `Layer`, `LayerBuilder`, `Identity`, `Stack`.
//!
//! These compose **anything** — `Layer<S>` carries no `Service` bound (ADR-0029
//! §3), so the same machinery composes an HTTP `Service` stack today and a WS
//! subscription stack tomorrow. The composition *unit* (`Layer` / `LayerBuilder`
//! / `Stack`) is shared; the assembled *product* is transport-specific (an HTTP
//! `Service`, a WS reconnect connector, …), which is why the output type is
//! named [`Layer::Wrapped`], not `Service`.
//!
//! # Ordering invariant
//!
//! The **first** `.layer()` call is permanently the outermost wrapper and
//! therefore the first to handle each request.
//!
//! ```
//! # use oath_adapter_net_api::compose::{Layer, LayerBuilder};
//! # struct TracingLayer;
//! # struct MetricsLayer;
//! # impl<S> Layer<S> for TracingLayer { type Wrapped = S; fn layer(&self, s: S) -> S { s } }
//! # impl<S> Layer<S> for MetricsLayer { type Wrapped = S; fn layer(&self, s: S) -> S { s } }
//! // TracingLayer is added first → outermost → wraps everything else.
//! let _svc = LayerBuilder::new()
//!     .layer(TracingLayer) // outermost
//!     .layer(MetricsLayer) // inner
//!     .wrap(());           // leaf: any value (a `Service` leaf lives in net-http-api)
//! ```

/// Wrap a value of type `S`, producing a new value that adds behaviour.
///
/// Typically a struct that holds configuration and owns an inner value. The
/// outer layer's [`Layer::layer`] method wraps the inner value, producing a
/// new value that adds the layer's behaviour.
pub trait Layer<S> {
    /// The wrapped type produced by this layer.
    ///
    /// Transport-neutral: it is an HTTP `Service` for an HTTP stack, a WS
    /// connector for a WS stack, and so on. The abstraction names the *result
    /// of wrapping*, never a specific transport's contract.
    type Wrapped;

    /// Wrap `inner` with this layer's behaviour.
    fn layer(&self, inner: S) -> Self::Wrapped;
}

/// Type-safe layer compositor.
///
/// Layers are applied in **declaration order**: the first `.layer()` call is
/// the outermost wrapper and therefore the first to execute on each request.
///
/// ```
/// # use oath_adapter_net_api::compose::{Identity, LayerBuilder};
/// let _builder = LayerBuilder::new(); // starts with Identity (no-op)
/// ```
#[derive(Debug, Clone)]
pub struct LayerBuilder<L> {
    layer: L,
}

impl Default for LayerBuilder<Identity> {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerBuilder<Identity> {
    /// Create a new builder with no layers applied.
    #[must_use]
    pub const fn new() -> Self {
        Self { layer: Identity }
    }
}

impl<L> LayerBuilder<L> {
    /// Add a new layer `New` into this `LayerBuilder`.
    ///
    /// `New` becomes the new `Inner`; the accumulated `L` remains `Outer` and
    /// therefore executes first on every request. This preserves the invariant
    /// that the **first** `.layer()` call stays permanently outermost.
    #[must_use]
    pub fn layer<New>(self, layer: New) -> LayerBuilder<Stack<New, L>> {
        LayerBuilder {
            layer: Stack {
                inner: layer,
                outer: self.layer,
            },
        }
    }

    /// Finalize the stack by wrapping a concrete leaf value.
    ///
    /// Consumes the builder and returns the fully composed value.
    /// The concrete type is fully resolved at compile time — no boxing, no
    /// `dyn`.
    pub fn wrap<S>(self, inner: S) -> L::Wrapped
    where
        L: Layer<S>,
    {
        self.layer.layer(inner)
    }
}

/// The no-op layer — passes the inner value through unchanged.
///
/// `Identity` is the initial state of a fresh [`LayerBuilder`].
#[derive(Debug, Clone)]
pub struct Identity;

impl<S> Layer<S> for Identity {
    type Wrapped = S;

    fn layer(&self, inner: S) -> S {
        inner
    }
}

/// Compose two [`Layer`] impls into one.
///
/// When assembling the stack, `Inner.layer(leaf)` is applied first, then
/// `Outer.layer(result)`. `Outer` is therefore the outermost wrapper and the
/// first to handle each request.
///
/// Because [`LayerBuilder::layer`] produces `Stack<New, L>` with `New` in
/// the `Inner` slot and the accumulated `L` in the `Outer` slot, each new
/// layer is nested *inside* the existing stack — leaving the first `.layer()`
/// call's layer permanently outermost.
#[derive(Debug, Clone)]
pub struct Stack<Inner, Outer> {
    inner: Inner,
    outer: Outer,
}

impl<S, Inner, Outer> Layer<S> for Stack<Inner, Outer>
where
    Inner: Layer<S>,
    Outer: Layer<Inner::Wrapped>,
{
    type Wrapped = Outer::Wrapped;

    fn layer(&self, value: S) -> Outer::Wrapped {
        // Apply Inner first (closer to the leaf), then wrap with Outer.
        let wrapped = self.inner.layer(value);
        self.outer.layer(wrapped)
    }
}
