//! Composition machinery: `Layer`, `LayerBuilder`, `Identity`, `Stack`.
//!
//! These compose **anything** — `Layer<T>` places no bound on `T`, so the same
//! machinery composes an HTTP `Service` stack today and a WebSocket subscription
//! stack tomorrow. The composition *unit* (`Layer` / `LayerBuilder` / `Stack`) is
//! shared; the assembled *product* is transport-specific (an HTTP `Service`, a WS
//! reconnect connector, …), which is why the output type is named
//! [`Layer::Wrapped`], not `Service`.
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
//! # impl<T> Layer<T> for TracingLayer { type Wrapped = T; fn layer(&self, inner: T) -> T { inner } }
//! # impl<T> Layer<T> for MetricsLayer { type Wrapped = T; fn layer(&self, inner: T) -> T { inner } }
//! // TracingLayer is added first → outermost → wraps everything else.
//! let _svc = LayerBuilder::new()
//!     .layer(TracingLayer) // outermost
//!     .layer(MetricsLayer) // inner
//!     .wrap(());           // leaf: any value (a `Service` leaf lives in net-http-api)
//! ```

/// Wrap a value of type `T`, producing a new value that adds behaviour.
///
/// Typically a struct that holds configuration and owns an inner value. The
/// outer layer's [`Layer::layer`] method wraps the inner value, producing a
/// new value that adds the layer's behaviour.
pub trait Layer<T> {
    /// The wrapped type produced by this layer.
    ///
    /// Transport-neutral: it is an HTTP `Service` for an HTTP stack, a WS
    /// connector for a WS stack, and so on. The abstraction names the *result
    /// of wrapping*, never a specific transport's contract.
    type Wrapped;

    /// Wrap `inner` with this layer's behaviour.
    fn layer(&self, inner: T) -> Self::Wrapped;
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
    #[must_use]
    pub fn wrap<T>(self, inner: T) -> L::Wrapped
    where
        L: Layer<T>,
    {
        self.layer.layer(inner)
    }
}

/// The no-op layer — passes the inner value through unchanged.
///
/// `Identity` is the initial state of a fresh [`LayerBuilder`].
#[derive(Debug, Clone)]
pub struct Identity;

impl<T> Layer<T> for Identity {
    type Wrapped = T;

    fn layer(&self, inner: T) -> T {
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

impl<T, Inner, Outer> Layer<T> for Stack<Inner, Outer>
where
    Inner: Layer<T>,
    Outer: Layer<Inner::Wrapped>,
{
    type Wrapped = Outer::Wrapped;

    fn layer(&self, value: T) -> Outer::Wrapped {
        // Apply Inner first (closer to the leaf), then wrap with Outer.
        let wrapped = self.inner.layer(value);
        self.outer.layer(wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::{Identity, Layer, LayerBuilder, Stack};
    use core::mem::size_of;

    // A layer that parenthesises a `String` under its tag, making the composition
    // nesting — and therefore the ordering invariant — directly observable:
    // `Tag("A").layer("x".into()) == "A(x)"`.
    struct Tag(&'static str);

    impl Layer<String> for Tag {
        type Wrapped = String;

        fn layer(&self, inner: String) -> String {
            format!("{}({})", self.0, inner)
        }
    }

    #[test]
    fn first_layer_is_outermost() {
        // `A` is added first, so it must end up outermost — wrapping `B`, which in
        // turn wraps the leaf. This is the module's load-bearing ordering invariant.
        let composed = LayerBuilder::new()
            .layer(Tag("A"))
            .layer(Tag("B"))
            .wrap(String::from("leaf"));
        assert_eq!(composed, "A(B(leaf))");
    }

    #[test]
    fn empty_builder_returns_the_leaf_unchanged() {
        // A fresh builder is just `Identity`, so it hands the leaf back as-is.
        let composed = LayerBuilder::new().wrap(String::from("leaf"));
        assert_eq!(composed, "leaf");
    }

    #[test]
    fn identity_in_the_middle_is_a_noop() {
        // An explicit `Identity` layer must not affect the composed result.
        let composed = LayerBuilder::new()
            .layer(Tag("A"))
            .layer(Identity)
            .layer(Tag("B"))
            .wrap(String::from("leaf"));
        assert_eq!(composed, "A(B(leaf))");
    }

    #[test]
    fn default_matches_new() {
        let via_default = LayerBuilder::<Identity>::default().wrap(String::from("x"));
        let via_new = LayerBuilder::new().wrap(String::from("x"));
        assert_eq!(via_default, via_new);
    }

    #[test]
    fn composition_machinery_is_zero_sized() {
        // The no-op layer, a fresh builder, and a stack of ZST layers all cost
        // zero bytes — the "no boxing, no dyn" promise made testable.
        assert_eq!(size_of::<Identity>(), 0);
        assert_eq!(size_of::<LayerBuilder<Identity>>(), 0);
        assert_eq!(size_of::<Stack<Identity, Identity>>(), 0);
    }
}
