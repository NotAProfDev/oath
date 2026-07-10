# net-http response-body bounds — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound a misbehaving venue response body on both axes — a per-frame *stall* timeout for streaming bodies (frees a wedged concurrency permit) and a *max-bytes* cap for buffered bodies (prevents OOM) — landing ADR-0034 Amendment #13.

**Architecture:** A new `TimeoutBody<B,T>` + `StallTimeoutLayer<T>` in `net-http-api` wraps streaming response bodies with a `Timer`-driven inactivity deadline, placed innermost in `stack()` so `Guarded` wraps it and a stall releases the permit. A new typed `LimitedBody<B>` (also `net-http-api`) caps the hyper leaf's buffered `collect()`. To store the sleep future inline (no `Box`/`dyn`), the `Timer` trait gains `type Sleep`. Overflow is a new non-retryable `HttpError::BodyTooLarge`.

**Tech Stack:** Rust 2024, `http-body` 1.x, `http-body-util`, `pin-project-lite`, `bytes`, the crate's runtime-neutral `Timer` seam + `MockTimer`, `tokio` (dev/test only).

**Design source:** [docs/superpowers/specs/2026-07-10-net-http-response-body-bounds-design.md](../specs/2026-07-10-net-http-response-body-bounds-design.md).

## Global Constraints

- Edition **2024**, MSRV **1.90** (`just msrv`).
- **No `unsafe`** (`unsafe_code = "deny"`); use `pin_project_lite` for pin projection.
- **No `unwrap`/`expect`/indexing in non-test code** — return `Result`, model errors with `thiserror`. Test code is exempt.
- **Document every public item** (`missing_docs` warned) — rustdoc + a doctest on each new public item.
- **Respect dependency direction:** `oath-adapter-net-api` ← `oath-adapter-net-http-api` ← `oath-adapter-net-http-hyper`. Never introduce a cycle.
- **Clippy `all` is deny-level.** No new warnings.
- **Conventional Commits** (commit-msg hook). Breaking pre-release changes use `!`.
- **Definition of done:** `just ci` green (fmt, lint, test, **doc**, deny, typos). Work happens on a worktree branch under `.claude/worktrees/<slug>` off `main`; one PR `Closes #N`.
- **CHANGELOG.md `[Unreleased]`** updated (Task 7).

---

## Task 1: `Timer::Sleep` associated type

Behaviour-preserving refactor that makes each `Timer` impl's sleep future *nameable*, so `TimeoutBody` (Task 3) can store it inline. No new behaviour — the safety net is that every existing timing suite must stay green and Task 3 must compile.

**Files:**
- Modify: `crates/adapter/net/api/src/timer.rs:12-19` (trait) + its `FixedTimer` test double (`:30-37`)
- Modify: `crates/adapter/net/http/hyper/src/timer.rs:15-18` (`TokioTimer`)
- Modify: `crates/adapter/net/mock/src/timer.rs:99-114` (`MockTimer`)

**Interfaces:**
- Produces: `trait Timer { type Sleep: Future<Output=()> + Send; fn sleep(&self, dur: Duration) -> Self::Sleep; fn now(&self) -> Instant; }`

- [ ] **Step 1: Add the associated type to the trait**

In `crates/adapter/net/api/src/timer.rs`, change the trait:

```rust
pub trait Timer: Clone + Send + Sync {
    /// The concrete future returned by [`sleep`](Timer::sleep). Named (not
    /// `impl Future`) so body wrappers can store it inline in a `#[pin]` field
    /// without boxing.
    type Sleep: Future<Output = ()> + Send;

    /// Complete after `dur` has elapsed.
    fn sleep(&self, dur: Duration) -> Self::Sleep;

    /// The current instant — for elapsed-time reads (token-bucket refill,
    /// circuit cooldown).
    fn now(&self) -> Instant;
}
```

And update the `FixedTimer` test double in the same file's `mod tests`:

```rust
impl Timer for FixedTimer {
    type Sleep = std::future::Ready<()>;
    fn sleep(&self, _dur: Duration) -> std::future::Ready<()> {
        std::future::ready(())
    }
    fn now(&self) -> Instant {
        self.0
    }
}
```

- [ ] **Step 2: Update `TokioTimer`**

In `crates/adapter/net/http/hyper/src/timer.rs`:

```rust
impl Timer for TokioTimer {
    type Sleep = tokio::time::Sleep;
    fn sleep(&self, dur: Duration) -> tokio::time::Sleep {
        tokio::time::sleep(dur)
    }

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
    }
}
```

Remove the now-unused `use std::future::Future;` if clippy flags it (it may still be referenced elsewhere in the file — only remove if `just lint` warns).

- [ ] **Step 3: Update `MockTimer`**

In `crates/adapter/net/mock/src/timer.rs`, change only the impl signature (the body already builds the named `Sleep` struct):

```rust
impl Timer for MockTimer {
    type Sleep = Sleep;
    fn sleep(&self, dur: Duration) -> Sleep {
        let deadline = {
            let state = lock(&self.state);
            state.now + dur
        };
        Sleep {
            state: Arc::clone(&self.state),
            deadline,
        }
    }

    fn now(&self) -> Instant {
        lock(&self.state).now
    }
}
```

- [ ] **Step 4: Verify every existing timing suite still passes**

Run: `cargo test -p oath-adapter-net-api -p oath-adapter-net-mock -p oath-adapter-net-http-api -p oath-adapter-net-http-hyper`
Expected: PASS — the trait change is behaviour-preserving; every `self.timer.sleep(dur).await` compiles and runs identically.

- [ ] **Step 5: Lint + doc**

Run: `just lint && just doc`
Expected: no warnings; rustdoc builds (the new `type Sleep` is documented).

- [ ] **Step 6: Commit**

```bash
git add crates/adapter/net/api/src/timer.rs crates/adapter/net/http/hyper/src/timer.rs crates/adapter/net/mock/src/timer.rs
git commit -m "refactor(net)!: add Timer::Sleep associated type for inline sleep storage"
```

---

## Task 2: `BodyTooLarge` error surface

Add the error kind + variant + telemetry label, and pin its non-retryability.

**Files:**
- Modify: `crates/adapter/net/api/src/error_kind.rs:14-42` (add `ErrorKind::BodyTooLarge`)
- Modify: `crates/adapter/net/http/api/src/error.rs:18-37,60-69` (variant + `HasErrorKind` arm)
- Modify: `crates/adapter/net/http/api/src/trace.rs:31-42` (label arm) + its test module
- Modify: `crates/adapter/net/http/api/src/retry.rs:410-418` (`err_of` arm) + a new test

**Interfaces:**
- Produces: `ErrorKind::BodyTooLarge`; `HttpError::BodyTooLarge` (unit); `kind_label(ErrorKind::BodyTooLarge) == "body_too_large"`.

- [ ] **Step 1: Write the failing label test**

In `crates/adapter/net/http/api/src/trace.rs`, inside `mod tests` add:

```rust
#[test]
fn body_too_large_has_a_stable_label() {
    assert_eq!(
        super::kind_label(oath_adapter_net_api::ErrorKind::BodyTooLarge),
        "body_too_large"
    );
}
```

- [ ] **Step 2: Run it — expect a compile failure**

Run: `cargo test -p oath-adapter-net-http-api trace::tests::body_too_large_has_a_stable_label`
Expected: FAIL to compile — `no variant named BodyTooLarge found for enum ErrorKind`.

- [ ] **Step 3: Add `ErrorKind::BodyTooLarge`**

In `crates/adapter/net/api/src/error_kind.rs`, add before the closing brace of the enum (after `CircuitOpen`):

```rust
    /// A response body exceeded the configured maximum size and was rejected
    /// before being fully buffered. A deliberate local decision, not a transport
    /// outcome; non-retryable.
    BodyTooLarge,
```

- [ ] **Step 4: Add `HttpError::BodyTooLarge` + its classification**

In `crates/adapter/net/http/api/src/error.rs`, add the variant to the enum (after `CircuitOpen`):

```rust
    /// A response body exceeded the configured maximum buffered size.
    #[error("response body exceeded the configured maximum")]
    BodyTooLarge,
```

and add the arm to `HasErrorKind::kind`:

```rust
            Self::BodyTooLarge => ErrorKind::BodyTooLarge,
```

Extend the existing `kind_maps_each_variant` test in `error.rs`'s `mod tests`:

```rust
        assert_eq!(HttpError::BodyTooLarge.kind(), ErrorKind::BodyTooLarge);
```

- [ ] **Step 5: Add the `kind_label` arm**

In `crates/adapter/net/http/api/src/trace.rs::kind_label`, add before the `_` arm:

```rust
        ErrorKind::BodyTooLarge => "body_too_large",
```

- [ ] **Step 6: Run the label + error tests — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api trace::tests::body_too_large_has_a_stable_label error::tests::kind_maps_each_variant`
Expected: PASS.

- [ ] **Step 7: Write the failing non-retry test**

In `crates/adapter/net/http/api/src/retry.rs`, inside `mod tests` add:

```rust
#[tokio::test]
async fn body_too_large_is_not_retried() {
    // BodyTooLarge is not a transient kind, so even an eligible request sends once.
    let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::BodyTooLarge)]);
    let svc = RetryLayer::new(
        cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
        MockTimer::new(),
    )
    .layer(leaf.clone());
    let err = svc.call(req(true)).await.unwrap_err();
    assert!(matches!(err, HttpError::BodyTooLarge));
    assert_eq!(leaf.calls(), 1, "BodyTooLarge is non-transient → never retried");
}
```

- [ ] **Step 8: Run it — expect a wrong-variant failure**

Run: `cargo test -p oath-adapter-net-http-api retry::tests::body_too_large_is_not_retried`
Expected: FAIL — `err_of` maps the kind through its `_ => HttpError::other("boom")` fallback, so `matches!(err, HttpError::BodyTooLarge)` is false.

- [ ] **Step 9: Map the kind in `err_of`**

In `crates/adapter/net/http/api/src/retry.rs::err_of`, add before the `_` arm:

```rust
            ErrorKind::BodyTooLarge => HttpError::BodyTooLarge,
```

- [ ] **Step 10: Run it — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api retry::tests::body_too_large_is_not_retried`
Expected: PASS — `is_transient(BodyTooLarge)` is false (only `Timeout`/`Connection` are), so the leaf is hit exactly once.

- [ ] **Step 11: Lint + doc + commit**

Run: `just lint && just doc`

```bash
git add crates/adapter/net/api/src/error_kind.rs crates/adapter/net/http/api/src/error.rs crates/adapter/net/http/api/src/trace.rs crates/adapter/net/http/api/src/retry.rs
git commit -m "feat(net): add BodyTooLarge error kind, variant, and label"
```

---

## Task 3: `TimeoutBody` + `StallTimeoutLayer`

The stall body wrapper and its layer, plus the permit-release proof (`Guarded` outside `TimeoutBody`).

**Files:**
- Create: `crates/adapter/net/http/api/src/stall.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs:48,65` (module decl + re-export)

**Interfaces:**
- Consumes: `Timer::Sleep` (Task 1); `HttpError::Timeout` (existing); `Guarded` (existing, from `body.rs`).
- Produces: `TimeoutBody::<B,T>::new(inner: B, timeout: Option<Duration>, timer: T)`; `StallTimeoutLayer::<T>::new(timeout: Option<Duration>, timer: T)`; the layer's `Wrapped = StallTimeout<S,T>` mapping `http::Response<B>` → `http::Response<TimeoutBody<B,T>>`.

- [ ] **Step 1: Register the module + create the file skeleton with the failing test**

In `crates/adapter/net/http/api/src/lib.rs`, add `pub mod stall;` (after `pub mod service;`) and `pub use stall::{StallTimeout, StallTimeoutLayer, TimeoutBody};` (after the `stack` re-export).

Create `crates/adapter/net/http/api/src/stall.rs` with the module doc, imports, and **only** the first test (so we can watch it fail):

```rust
//! The `StallTimeout` body-inactivity layer (ADR-0034 Amendment #13).
//!
//! Wraps a *streaming* response body with a per-frame **inactivity** deadline via
//! the runtime-neutral [`Timer`] seam: if no frame arrives within the configured
//! duration the body yields [`HttpError::Timeout`], so a stalled transfer can no
//! longer wedge a `Guarded` concurrency permit. Placed innermost in `stack()` so
//! `Guarded` wraps [`TimeoutBody`] and the stall error releases the permit.
//! Inert on buffered bodies (one ready frame) and when the deadline is `None`.

use crate::{HttpError, Service};
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use oath_adapter_net_api::{Layer, Timer};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::TimeoutBody;
    use crate::HttpError;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use oath_adapter_net_mock::MockTimer;
    use std::collections::VecDeque;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    // A body that never yields a frame — models a wedged/stalled transfer.
    struct PendingBody;
    impl Body for PendingBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Pending
        }
        fn is_end_stream(&self) -> bool {
            false
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }

    #[tokio::test]
    async fn stall_fires_when_no_frame_arrives() {
        let timer = MockTimer::new();
        let body = TimeoutBody::new(PendingBody, Some(Duration::from_secs(1)), timer.clone());
        let waiter = tokio::spawn(async move {
            let mut body = pin!(body);
            std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await
        });
        tokio::task::yield_now().await; // arms the deadline, inner pends
        timer.advance(Duration::from_secs(1)); // fire the stall deadline
        let frame = waiter.await.unwrap();
        assert!(
            matches!(frame, Some(Err(HttpError::Timeout))),
            "a stalled body must yield Timeout"
        );
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure**

Run: `cargo test -p oath-adapter-net-http-api stall::tests::stall_fires_when_no_frame_arrives`
Expected: FAIL to compile — `TimeoutBody` is not defined.

- [ ] **Step 3: Implement `TimeoutBody`**

Add to `stall.rs` (above the `#[cfg(test)]` module):

```rust
pin_project_lite::pin_project! {
    /// A response body wrapper enforcing a per-frame **inactivity** timeout.
    ///
    /// On each poll it (re-)arms a [`Timer::sleep`] deadline; if the deadline
    /// fires before the inner body produces a frame, it yields
    /// [`HttpError::Timeout`]. Each frame resets the deadline (lazily: `sleep` is
    /// cleared and re-armed on the next poll). A `None` timeout is fully inert —
    /// the wrapper forwards to `inner` unchanged. Forwards `is_end_stream` and
    /// `size_hint` (ADR-0034 §2 transparency).
    pub struct TimeoutBody<B, T: Timer> {
        #[pin]
        inner: B,
        #[pin]
        sleep: Option<T::Sleep>,
        timeout: Option<Duration>,
        timer: T,
    }
}

impl<B, T: Timer> TimeoutBody<B, T> {
    /// Wrap `inner` with a stall timeout. `None` disables it (pass-through).
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::TimeoutBody;
    /// use oath_adapter_net_mock::MockTimer;
    /// use http_body_util::Empty;
    /// use bytes::Bytes;
    /// use std::time::Duration;
    ///
    /// let _body = TimeoutBody::new(
    ///     Empty::<Bytes>::new(),
    ///     Some(Duration::from_secs(30)),
    ///     MockTimer::new(),
    /// );
    /// ```
    #[must_use]
    pub const fn new(inner: B, timeout: Option<Duration>, timer: T) -> Self {
        Self {
            inner,
            sleep: None,
            timeout,
            timer,
        }
    }
}

impl<B: fmt::Debug, T: Timer> fmt::Debug for TimeoutBody<B, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeoutBody")
            .field("inner", &self.inner)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<B, T> Body for TimeoutBody<B, T>
where
    B: Body<Data = Bytes, Error = HttpError>,
    T: Timer,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let mut this = self.project();
        let Some(dur) = *this.timeout else {
            // Disabled: fully transparent.
            return this.inner.poll_frame(cx);
        };
        // Arm the deadline if unset.
        if this.sleep.is_none() {
            let nap = this.timer.sleep(dur);
            this.sleep.set(Some(nap));
        }
        // Poll the timer FIRST so its waker stays registered while the body pends.
        if let Some(sleep) = this.sleep.as_mut().as_pin_mut() {
            if sleep.poll(cx).is_ready() {
                return Poll::Ready(Some(Err(HttpError::Timeout)));
            }
        }
        let frame = ready!(this.inner.poll_frame(cx));
        this.sleep.set(None); // lazy per-frame reset (inactivity semantics)
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
```

- [ ] **Step 4: Run the stall test — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api stall::tests::stall_fires_when_no_frame_arrives`
Expected: PASS.

- [ ] **Step 5: Add the reset / inert / transparency tests**

Add to `stall.rs`'s `mod tests` (the `Frames`/`Stub` doubles mirror `body.rs`):

```rust
    // A multi-frame body; every frame is immediately ready.
    struct Frames {
        frames: VecDeque<Bytes>,
    }
    impl Frames {
        fn new<const N: usize>(frames: [&'static [u8]; N]) -> Self {
            Self {
                frames: frames.iter().copied().map(Bytes::from_static).collect(),
            }
        }
    }
    impl Body for Frames {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            let this = self.get_mut();
            Poll::Ready(this.frames.pop_front().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.frames.iter().map(|f| f.len() as u64).sum())
        }
    }

    #[test]
    fn each_frame_resets_the_deadline() {
        // timeout = 1s; two 500ms gaps (1s total) must NOT trip, because each
        // per-frame gap is < 1s. A no-reset impl (deadline armed once) WOULD trip
        // once cumulative time reaches 1s.
        let timer = MockTimer::new();
        let body = TimeoutBody::new(Frames::new([b"a", b"b"]), Some(Duration::from_secs(1)), timer.clone());
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Ready(Some(Ok(_))))); // frame a @ t=0
        timer.advance(Duration::from_millis(500));
        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Ready(Some(Ok(_))))); // frame b @ t=0.5s
        timer.advance(Duration::from_millis(500)); // t=1.0s total
        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Ready(None))); // end, NOT a stall
    }

    #[tokio::test]
    async fn none_timeout_is_inert() {
        // A None timeout never arms a deadline: a pending body just stays pending
        // even after the clock advances arbitrarily.
        let timer = MockTimer::new();
        let body = TimeoutBody::new(PendingBody, None, timer.clone());
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        timer.advance(Duration::from_secs(3600));
        assert!(
            body.as_mut().poll_frame(&mut cx).is_pending(),
            "None disables the stall timeout entirely"
        );
    }

    #[test]
    fn forwards_is_end_stream_and_size_hint() {
        let inner = Frames::new([b"ab", b"cde"]);
        let ref_hint = inner.size_hint().exact();
        let wrapped =
            TimeoutBody::new(Frames::new([b"ab", b"cde"]), Some(Duration::from_secs(1)), MockTimer::new());
        assert_eq!(wrapped.size_hint().exact(), ref_hint); // NOT silently unbounded
        assert!(!wrapped.is_end_stream());
        let ended =
            TimeoutBody::new(Frames::new([]), Some(Duration::from_secs(1)), MockTimer::new());
        assert!(ended.is_end_stream()); // forwarded, not the `false` default
    }
```

- [ ] **Step 6: Run the new body tests — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api stall::tests`
Expected: PASS (all four body tests).

- [ ] **Step 7: Add the permit-release test (the crux: `Guarded` outside `TimeoutBody`)**

Add to `stall.rs`'s `mod tests` (add `use crate::Guarded;` and `use async_lock::Semaphore;` and `use std::sync::Arc;` to the test imports):

```rust
    // A stall must release the concurrency permit: with Guarded OUTSIDE TimeoutBody,
    // the stall error frame is a non-Ok frame that Guarded observes and releases on.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the test holds the guarded body across polls to prove release at the stall while still alive"
    )]
    #[test]
    fn stall_releases_the_concurrency_permit() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.try_acquire_arc().expect("permit free at start");
        let timer = MockTimer::new();
        let inner = TimeoutBody::new(PendingBody, Some(Duration::from_secs(1)), timer.clone());
        let body = Guarded::new(inner, Some(permit));
        let mut body = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());

        assert!(body.as_mut().poll_frame(&mut cx).is_pending()); // arms deadline, permit held
        assert!(sem.try_acquire_arc().is_none(), "permit held while streaming");

        timer.advance(Duration::from_secs(1)); // fire the stall
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Err(HttpError::Timeout)))
        ));
        assert!(
            sem.try_acquire_arc().is_some(),
            "the stall error must release the permit"
        );
    }
```

- [ ] **Step 8: Run it — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api stall::tests::stall_releases_the_concurrency_permit`
Expected: PASS.

- [ ] **Step 9: Implement `StallTimeoutLayer` + `StallTimeout` and a transparency test**

Add to `stall.rs` (below `TimeoutBody`, above the tests):

```rust
/// The [`StallTimeout`] [`Layer`] factory: holds the (optional) inactivity
/// deadline + clock and wraps any inner service's response body in
/// [`TimeoutBody`].
pub struct StallTimeoutLayer<T> {
    timeout: Option<Duration>,
    timer: T,
}

impl<T> StallTimeoutLayer<T> {
    /// Build the layer. `None` disables the stall timeout (pass-through body).
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::StallTimeoutLayer;
    /// use oath_adapter_net_mock::MockTimer;
    /// use std::time::Duration;
    ///
    /// let _layer = StallTimeoutLayer::new(Some(Duration::from_secs(30)), MockTimer::new());
    /// ```
    #[must_use]
    pub const fn new(timeout: Option<Duration>, timer: T) -> Self {
        Self { timeout, timer }
    }
}

impl<T: Clone> Clone for StallTimeoutLayer<T> {
    fn clone(&self) -> Self {
        Self {
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for StallTimeoutLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StallTimeoutLayer")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for StallTimeoutLayer<T> {
    type Wrapped = StallTimeout<S, T>;
    fn layer(&self, inner: S) -> StallTimeout<S, T> {
        StallTimeout {
            inner,
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

/// The `StallTimeout` middleware: wraps the inner service's response body in a
/// [`TimeoutBody`]. The send itself is untouched (that is the `Timeout` layer's
/// job); only the streamed body gains the inactivity deadline.
pub struct StallTimeout<S, T> {
    inner: S,
    timeout: Option<Duration>,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for StallTimeout<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            timeout: self.timeout,
            timer: self.timer.clone(),
        }
    }
}

impl<S, T> fmt::Debug for StallTimeout<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StallTimeout")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for StallTimeout<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Response = http::Response<TimeoutBody<B, T>>;
    type Error = HttpError;

    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let resp = self.inner.call(req).await?;
            let (parts, body) = resp.into_parts();
            let body = TimeoutBody::new(body, self.timeout, self.timer.clone());
            Ok(http::Response::from_parts(parts, body))
        }
    }
}
```

Add a transparency test to `mod tests` (a fast leaf's body passes through the layer intact — mirror `timeout.rs`'s `FastLeaf`/`StubBody` doubles inline):

```rust
    use crate::Service;
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Layer;
    use std::future::Future;

    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl Body for StubBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.data.is_none()
        }
        fn size_hint(&self) -> SizeHint {
            self.data.as_ref().map_or_else(
                || SizeHint::with_exact(0),
                |d| SizeHint::with_exact(d.len() as u64),
            )
        }
    }
    #[derive(Clone)]
    struct FastLeaf;
    impl Service<http::Request<Bytes>> for FastLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async {
                Ok(http::Response::new(StubBody {
                    data: Some(Bytes::from_static(b"ok")),
                }))
            }
        }
    }

    #[tokio::test]
    async fn layer_wraps_and_body_streams_through() {
        use super::StallTimeoutLayer;
        let svc =
            StallTimeoutLayer::new(Some(Duration::from_secs(30)), MockTimer::new()).layer(FastLeaf);
        let resp = svc
            .call(http::Request::new(Bytes::new()))
            .await
            .expect("fast leaf");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // wrapped, streamed through untouched
    }
```

- [ ] **Step 10: Run the full stall suite + lint + doc**

Run: `cargo test -p oath-adapter-net-http-api stall::tests && just lint && just doc`
Expected: PASS; no warnings; doctests for `TimeoutBody::new`/`StallTimeoutLayer::new` build and run.

- [ ] **Step 11: Commit**

```bash
git add crates/adapter/net/http/api/src/stall.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): TimeoutBody + StallTimeoutLayer for streaming body stall timeouts"
```

---

## Task 4: Wire `StallTimeoutLayer` into `stack()` + `body_stall_timeout` config

**Files:**
- Modify: `crates/adapter/net/http/api/src/stack.rs` — `HttpConfig` field + `Debug` + `validate_config` + composition + doctest + `http_cfg` test helper + a full-stack test
- Modify: `crates/adapter/net/http/hyper/src/build.rs` — doctest `HttpConfig` + `http_cfg` test helper
- Modify: `crates/adapter/net/http/hyper/examples/client_with_directives.rs` — `HttpConfig` literal

**Interfaces:**
- Consumes: `StallTimeoutLayer` (Task 3).
- Produces: `HttpConfig.body_stall_timeout: Option<Duration>`; `stack()` composes `.layer(StallTimeoutLayer::new(cfg.body_stall_timeout, timer))` innermost; `BuildError::ZeroDuration("body_stall_timeout")` when `Some(ZERO)`.

- [ ] **Step 1: Write the failing validation test**

In `crates/adapter/net/http/api/src/stack.rs`'s `mod tests`, add:

```rust
#[test]
fn zero_body_stall_timeout_is_rejected_at_build() {
    let timer = MockTimer::new();
    let leaf = ScriptLeaf::new(timer.clone(), vec![Step::Status(200)]);
    let mut cfg = http_cfg(1, Duration::from_secs(1), Duration::ZERO);
    cfg.body_stall_timeout = Some(Duration::ZERO);
    let Err(err) = stack(leaf, cfg, timer, NoAuth, rate_cfg()) else {
        panic!("Some(ZERO) body_stall_timeout must be a BuildError");
    };
    assert_eq!(err, BuildError::ZeroDuration("body_stall_timeout"));
}
```

- [ ] **Step 2: Run it — expect a compile failure**

Run: `cargo test -p oath-adapter-net-http-api stack::tests::zero_body_stall_timeout_is_rejected_at_build`
Expected: FAIL to compile — `HttpConfig` has no field `body_stall_timeout`.

- [ ] **Step 3: Add the `HttpConfig` field + `Debug` + validation**

In `crates/adapter/net/http/api/src/stack.rs`, add to `HttpConfig` (after `rate_limit_max_wait`):

```rust
    /// Per-frame inactivity deadline for a **streaming** response body; `None`
    /// disables it. Bounds a mid-transfer stall so a slow body cannot pin a
    /// concurrency permit indefinitely (ADR-0034 Amendment #13). Inert on
    /// buffered responses.
    pub body_stall_timeout: Option<Duration>,
```

Add to the manual `Debug` impl (after the `rate_limit_max_wait` field):

```rust
            .field("body_stall_timeout", &self.body_stall_timeout)
```

Add to `validate_config` (after the `retry_after_cap` zero check, before the threshold checks):

```rust
    if cfg.body_stall_timeout == Some(Duration::ZERO) {
        return Err(BuildError::ZeroDuration("body_stall_timeout"));
    }
```

- [ ] **Step 4: Fix the two in-crate `HttpConfig` constructors**

In `stack.rs`, add `body_stall_timeout: Some(Duration::from_secs(30)),` to the `HttpConfig` in the `stack` doctest (the `let cfg = HttpConfig { … }` block) and to the `http_cfg` test helper's `HttpConfig`.

- [ ] **Step 5: Run the validation test — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api stack::tests::zero_body_stall_timeout_is_rejected_at_build`
Expected: PASS.

- [ ] **Step 6: Write the failing full-stack stall test**

In `stack.rs`'s `mod tests`, add a streaming stalling leaf + test. First extend the `Step` enum with a `StreamStall` variant and its `ScriptLeaf` arm:

In the `enum Step` add `StreamStall,` and in `ScriptLeaf::call`'s `match step` add:

```rust
                    Step::StreamStall => Ok(http::Response::new(StubBody { data: None }.pending())),
```

That requires a pending body; simpler — add a dedicated streaming-pending body double and a `Step::StreamStall` arm returning it. Add this body to `mod tests`:

```rust
    // A streaming body that never yields a frame (models a mid-transfer stall).
    #[derive(Debug)]
    struct StallingBody;
    impl Body for StallingBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Pending
        }
        fn is_end_stream(&self) -> bool {
            false
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }
```

Because `ScriptLeaf::Response` is `http::Response<StubBody>`, the stall leaf needs its own type. Add a separate minimal leaf rather than overloading `ScriptLeaf`:

```rust
    #[derive(Clone)]
    struct StallLeaf;
    impl Service<http::Request<Bytes>> for StallLeaf {
        type Response = http::Response<StallingBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async { Ok(http::Response::new(StallingBody)) }
        }
    }

    #[tokio::test]
    async fn a_stalled_streaming_body_times_out_through_the_stack() {
        let timer = MockTimer::new();
        let mut cfg = http_cfg(1, Duration::from_secs(3600), Duration::ZERO);
        cfg.body_stall_timeout = Some(Duration::from_secs(1)); // short body-stall bound
        let svc = stack(StallLeaf, cfg, timer.clone(), NoAuth, rate_cfg()).expect("total config");

        // The send returns at headers immediately; the stall bites while draining.
        let resp = svc.call(req(RateScope::Global)).await.expect("headers arrive");
        let waiter = tokio::spawn(async move {
            let mut body = std::pin::pin!(resp.into_body());
            std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await
        });
        tokio::task::yield_now().await; // body registers the stall deadline
        timer.advance(Duration::from_secs(1)); // fire it
        let frame = waiter.await.unwrap();
        assert!(
            matches!(frame, Some(Err(HttpError::Timeout))),
            "a stalled streaming body must surface Timeout when drained"
        );
    }
```

(Remove the abandoned `Step::StreamStall` sketch — the standalone `StallLeaf` is cleaner. Ensure `mod tests` imports `http_body::{Body, Frame, SizeHint}` and `std::task::{Context, Poll}` and `std::pin::Pin`; they are already imported for the existing `StubBody`.)

- [ ] **Step 7: Run it — expect a compile failure (StallTimeoutLayer not yet wired)**

Run: `cargo test -p oath-adapter-net-http-api stack::tests::a_stalled_streaming_body_times_out_through_the_stack`
Expected: FAIL — the stall layer is not composed, so the body is `Guarded<StallingBody>` (never times out; the spawned poll hangs and the test would not observe `Timeout`). It may compile but hang/deadlock, or fail the assertion after the advance. Confirm it does **not** pass.

- [ ] **Step 8: Wire `StallTimeoutLayer` into `stack()`**

In `stack.rs`, add `StallTimeoutLayer` to the `use crate::{…}` import list, and change the `LayerBuilder` chain so the stall layer is innermost:

```rust
    let svc = LayerBuilder::new()
        .layer(TracingLayer::new(timer.clone())) // outermost
        .layer(CircuitBreakerLayer::new(cfg.circuit_breaker, timer.clone()))
        .layer(RetryLayer::new(cfg.retry, timer.clone()))
        .layer(rate)
        .layer(TimeoutLayer::new(cfg.timeout, timer.clone()))
        .layer(StallTimeoutLayer::new(cfg.body_stall_timeout, timer)) // innermost
        .wrap(inner);
```

(Note the `TimeoutLayer::new(cfg.timeout, timer.clone())` now clones; the final `timer` moves into `StallTimeoutLayer`.)

- [ ] **Step 9: Run the full-stack stall test + the whole stack suite — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api stack::tests`
Expected: PASS — including the existing ordering/permit tests (the new innermost layer is body-transparent to the send path).

- [ ] **Step 10: Fix the remaining `HttpConfig` construction sites**

Add `body_stall_timeout: Some(Duration::from_secs(30)),` to:
- `crates/adapter/net/http/hyper/src/build.rs` — the `build` doctest's `HttpConfig` literal and the `http_cfg()` test helper.
- `crates/adapter/net/http/hyper/examples/client_with_directives.rs` — the `HttpConfig` literal.

- [ ] **Step 11: Full verification + commit**

Run: `just check && just lint && just test && just doc`
Expected: PASS everywhere (all crates compile with the new required field; doctests updated).

```bash
git add crates/adapter/net/http/api/src/stack.rs crates/adapter/net/http/hyper/src/build.rs crates/adapter/net/http/hyper/examples/client_with_directives.rs
git commit -m "feat(net)!: wire StallTimeoutLayer into stack() + HttpConfig.body_stall_timeout"
```

---

## Task 5: `LimitedBody<B>` typed max-bytes wrapper

**Files:**
- Modify: `crates/adapter/net/http/api/src/body.rs` (add `LimitedBody` + tests)
- Modify: `crates/adapter/net/http/api/src/lib.rs:52` (re-export)

**Interfaces:**
- Consumes: `HttpError::BodyTooLarge` (Task 2).
- Produces: `LimitedBody::<B>::new(inner: B, max_bytes: u64)`; a `Body<Data=Bytes, Error=HttpError>` that emits `HttpError::BodyTooLarge` once cumulative DATA bytes exceed `max_bytes`.

- [ ] **Step 1: Write the failing tests**

In `crates/adapter/net/http/api/src/body.rs`'s `mod tests`, add (`Frames` and `Stub` doubles already exist in this module; add `use http_body_util::BodyExt;` to the test imports):

```rust
    #[tokio::test]
    async fn limited_body_passes_under_cap() {
        let body = LimitedBody::new(Frames::new([b"ab", b"cde"]), 10);
        let bytes = body.collect().await.unwrap().to_bytes();
        assert_eq!(bytes, Bytes::from_static(b"abcde"));
    }

    #[tokio::test]
    async fn limited_body_passes_at_exact_cap() {
        // 2 + 3 == 5; each frame's len is not > remaining, so the boundary passes.
        let body = LimitedBody::new(Frames::new([b"ab", b"cde"]), 5);
        let bytes = body.collect().await.unwrap().to_bytes();
        assert_eq!(bytes, Bytes::from_static(b"abcde"));
    }

    #[tokio::test]
    async fn limited_body_errors_over_cap() {
        // cap 4: "abc" (3) ok → remaining 1; "def" (3) > 1 → BodyTooLarge.
        let body = LimitedBody::new(Frames::new([b"abc", b"def"]), 4);
        let err = body.collect().await.expect_err("must overflow");
        assert!(matches!(err, HttpError::BodyTooLarge));
    }

    #[test]
    fn limited_body_clamps_size_hint_and_forwards_is_end_stream() {
        // inner exact = 100, cap 10 → clamp to exact 10 (lower >= remaining path).
        let wrapped = LimitedBody::new(Stub { remaining: 100 }, 10);
        assert_eq!(wrapped.size_hint().exact(), Some(10));
        assert!(!wrapped.is_end_stream());
        let ended = LimitedBody::new(Stub { remaining: 0 }, 10);
        assert!(ended.is_end_stream()); // forwarded
    }
```

- [ ] **Step 2: Run them — expect a compile failure**

Run: `cargo test -p oath-adapter-net-http-api body::tests::limited_body_passes_under_cap`
Expected: FAIL to compile — `LimitedBody` is not defined.

- [ ] **Step 3: Implement `LimitedBody`**

In `body.rs`, add (near `Guarded`; the file already imports `Body, Frame, SizeHint`, `Bytes`, `ready`, `Pin`, `Context`, `Poll`):

```rust
pin_project_lite::pin_project! {
    /// Wraps a response body, failing with [`HttpError::BodyTooLarge`] once the
    /// cumulative DATA-frame bytes exceed `remaining`. A **typed** alternative to
    /// `http_body_util::Limited` (which boxes its error): the whole HTTP stack
    /// keeps one concrete `HttpError` for service *and* body. Forwards
    /// `is_end_stream`; clamps `size_hint` to `remaining` (ADR-0034 §2), so a
    /// downstream collector stays bounded.
    pub struct LimitedBody<B> {
        #[pin]
        inner: B,
        remaining: u64,
    }
}

impl<B> LimitedBody<B> {
    /// Wrap `inner`, rejecting once cumulative DATA bytes exceed `max_bytes`.
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::LimitedBody;
    /// use http_body_util::Empty;
    /// use bytes::Bytes;
    ///
    /// let _body = LimitedBody::new(Empty::<Bytes>::new(), 16 * 1024 * 1024);
    /// ```
    #[must_use]
    pub const fn new(inner: B, max_bytes: u64) -> Self {
        Self {
            inner,
            remaining: max_bytes,
        }
    }
}

impl<B: fmt::Debug> fmt::Debug for LimitedBody<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedBody")
            .field("inner", &self.inner)
            .field("remaining", &self.remaining)
            .finish()
    }
}

impl<B> Body for LimitedBody<B>
where
    B: Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
        let this = self.project();
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    let len = data.len() as u64;
                    if len > *this.remaining {
                        *this.remaining = 0;
                        return Poll::Ready(Some(Err(HttpError::BodyTooLarge)));
                    }
                    *this.remaining -= len;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            // Terminal None or an inner error: pass through unchanged.
            other => Poll::Ready(other),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        // Mirror http_body_util::Limited's clamp so lower never exceeds upper.
        let mut hint = self.inner.size_hint();
        let n = self.remaining;
        if hint.lower() >= n {
            hint.set_exact(n);
        } else if let Some(max) = hint.upper() {
            hint.set_upper(n.min(max));
        } else {
            hint.set_upper(n);
        }
        hint
    }
}
```

Add `LimitedBody` to the `body::` re-export in `lib.rs`: `pub use body::{BufferMode, Guarded, LimitedBody, ResponseBody};`.

- [ ] **Step 4: Run the tests — expect PASS**

Run: `cargo test -p oath-adapter-net-http-api body::tests`
Expected: PASS (all four `limited_body_*` tests + the existing body tests).

- [ ] **Step 5: Lint + doc + commit**

Run: `just lint && just doc`

```bash
git add crates/adapter/net/http/api/src/body.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): LimitedBody — typed max-bytes response-body wrapper"
```

---

## Task 6: Cap the leaf's buffered collect (`ConnConfig::max_response_bytes`)

**Files:**
- Modify: `crates/adapter/net/http/hyper/src/leaf.rs` — `ConnConfig` field, `HyperLeaf` carries the cap, `Buffer` arm, `test_conn`, new server doubles + tests
- Modify: `crates/adapter/net/http/hyper/src/build.rs` — doctest `ConnConfig` + `conn()` test helper
- Modify: `crates/adapter/net/http/hyper/examples/client_with_directives.rs` — `ConnConfig` literal

**Interfaces:**
- Consumes: `LimitedBody` (Task 5), `HttpError::BodyTooLarge` (Task 2).
- Produces: `ConnConfig.max_response_bytes: Option<usize>`; `BufferMode::Buffer` responses are capped.

- [ ] **Step 1: Write the failing "honest oversized Content-Length" test**

In `crates/adapter/net/http/hyper/src/leaf.rs`'s `mod tests`, add a fixed-body server + the test (`spawn_echo_server` already exists; add a sized variant):

```rust
    // Serves a body of `n` bytes with an explicit Content-Length (via Full<Bytes>).
    async fn spawn_body_server(n: usize) -> String {
        let payload = Bytes::from(vec![b'x'; n]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let payload = payload.clone();
                let io = hyper_util::rt::TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |_r| {
                        let payload = payload.clone();
                        async move {
                            Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                                payload,
                            )))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn buffer_rejects_an_oversized_content_length_upfront() {
        let base = spawn_body_server(64).await;
        let conn = ConnConfig {
            max_response_bytes: Some(16),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/big"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let err = leaf.call(req).await.expect_err("64-byte body over a 16-byte cap");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::BodyTooLarge),
            "expected BodyTooLarge, got {err:?}"
        );
    }
```

- [ ] **Step 2: Run it — expect a compile failure**

Run: `cargo test -p oath-adapter-net-http-hyper leaf::tests::buffer_rejects_an_oversized_content_length_upfront`
Expected: FAIL to compile — `ConnConfig` has no field `max_response_bytes`.

- [ ] **Step 3: Add the `ConnConfig` field + carry it on `HyperLeaf`**

In `leaf.rs`, add to `ConnConfig` (after `http2_keep_alive_while_idle`):

```rust
    /// Maximum bytes to buffer for a `BufferMode::Buffer` response; `None` =
    /// unbounded. Rejects an oversized body with [`HttpError::BodyTooLarge`]
    /// (ADR-0034 Amendment #13) — memory-safety for a misbehaving venue.
    pub max_response_bytes: Option<usize>,
```

Add the field to `HyperLeaf` and thread it through `hyper_leaf`:

```rust
#[derive(Clone)]
pub struct HyperLeaf {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    inflight: Arc<InFlight>,
    max_response_bytes: Option<usize>,
}
```

In `hyper_leaf`, capture the cap before `conn` is moved and set it on the returned leaf:

```rust
pub fn hyper_leaf(conn: ConnConfig) -> HyperLeaf {
    let max_response_bytes = conn.max_response_bytes;
    // … existing connector/client construction (unchanged) …
    HyperLeaf {
        client,
        inflight: Arc::new(InFlight::default()),
        max_response_bytes,
    }
}
```

- [ ] **Step 4: Apply the cap in the `Buffer` arm**

In `HyperLeaf::call`, replace the `BufferMode::Buffer` arm. First bind the cap before `async move` (it borrows `&self`):

```rust
        let guard = InFlightGuard::enter(&self.inflight);
        let max_response_bytes = self.max_response_bytes;
        async move {
            let _guard = guard;
            // … unchanged: mode, into_parts, request, map_legacy_err, into_parts …
            let body = match mode {
                BufferMode::Stream => {
                    let mapper: fn(hyper::Error) -> HttpError = map_hyper_err;
                    ResponseBody::streaming(incoming.map_err(mapper))
                },
                BufferMode::Buffer => {
                    let bytes = match max_response_bytes {
                        Some(cap) => {
                            let cap = cap as u64;
                            if incoming.size_hint().upper().is_some_and(|u| u > cap) {
                                return Err(HttpError::BodyTooLarge);
                            }
                            LimitedBody::new(incoming.map_err(map_hyper_err), cap)
                                .collect()
                                .await?
                                .to_bytes()
                        },
                        None => incoming.collect().await.map_err(map_hyper_err)?.to_bytes(),
                    };
                    ResponseBody::buffered(bytes)
                },
            };
            Ok(http::Response::from_parts(parts, body))
        }
```

Add imports at the top of `leaf.rs`: `LimitedBody` to the `oath_adapter_net_http_api::{…}` use, and `http_body::Body` for `size_hint()` (add `use http_body::Body;`). `BodyExt::collect` is already available via `http_body_util::BodyExt` (add it to the `use http_body_util::{…}` line).

- [ ] **Step 5: Add `max_response_bytes` to `test_conn` and run the upfront test — expect PASS**

Add `max_response_bytes: None,` to `test_conn()` in `leaf.rs` (default unbounded for the existing tests), then set `Some(16)` in the new test's override (already written in Step 1).

Run: `cargo test -p oath-adapter-net-http-hyper leaf::tests::buffer_rejects_an_oversized_content_length_upfront`
Expected: PASS.

- [ ] **Step 6: Add the streaming-overflow (unsized/chunked) test**

Add a chunked server (no Content-Length → the upfront check passes; `LimitedBody` catches it while collecting):

```rust
    // Sends a chunked (no Content-Length) body of two `chunk`-sized pieces.
    async fn spawn_chunked_server(chunk: &'static [u8], count: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0_u8; 256];
            loop {
                let n = stream.read(&mut tmp).await.unwrap();
                assert!(n > 0, "peer closed before a full request head");
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            for _ in 0..count {
                stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .unwrap();
                stream.write_all(chunk).await.unwrap();
                stream.write_all(b"\r\n").await.unwrap();
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
            stream.flush().await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn buffer_caps_an_unsized_streaming_body_while_collecting() {
        // Two 8-byte chunks = 16 bytes, no Content-Length, over a 10-byte cap.
        let base = spawn_chunked_server(b"12345678", 2).await;
        let conn = ConnConfig {
            max_response_bytes: Some(10),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/chunked"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let err = leaf
            .call(req)
            .await
            .expect_err("16 chunked bytes over a 10-byte cap");
        assert!(
            matches!(err, oath_adapter_net_http_api::HttpError::BodyTooLarge),
            "expected BodyTooLarge from the streaming cap, got {err:?}"
        );
    }
```

- [ ] **Step 7: Add the under-cap and `None`-unbounded tests**

```rust
    #[tokio::test]
    async fn buffer_under_cap_collects_normally() {
        let base = spawn_body_server(8).await;
        let conn = ConnConfig {
            max_response_bytes: Some(1024),
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/small"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let resp = leaf.call(req).await.expect("8-byte body under a 1KiB cap");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 8);
    }

    #[tokio::test]
    async fn buffer_with_no_cap_is_unbounded() {
        let base = spawn_body_server(64).await;
        let conn = ConnConfig {
            max_response_bytes: None,
            ..test_conn()
        };
        let leaf = hyper_leaf(conn);
        let mut req = http::Request::get(format!("{base}/big"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);
        let resp = leaf.call(req).await.expect("no cap → unbounded");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 64);
    }
```

- [ ] **Step 8: Run the leaf cap suite — expect PASS**

Run: `cargo test -p oath-adapter-net-http-hyper leaf::tests`
Expected: PASS (upfront reject, chunked streaming cap, under-cap, no-cap, plus all existing leaf tests).

- [ ] **Step 9: Fix the remaining `ConnConfig` construction sites**

Add `max_response_bytes: Some(16 * 1024 * 1024),` to:
- `build.rs` — the `build` doctest's `ConnConfig` literal and the `conn()` test helper.
- `examples/client_with_directives.rs` — the `ConnConfig` literal.

- [ ] **Step 10: Full verification + commit**

Run: `just check && just lint && just test && just doc`
Expected: PASS everywhere.

```bash
git add crates/adapter/net/http/hyper/src/leaf.rs crates/adapter/net/http/hyper/src/build.rs crates/adapter/net/http/hyper/examples/client_with_directives.rs
git commit -m "feat(net)!: cap buffered response collect via ConnConfig::max_response_bytes"
```

---

## Task 7: ADR-0034 Amendment #13 + CHANGELOG

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md` (append Amendment #13)
- Modify: `CHANGELOG.md` (`[Unreleased]`)
- Move: `docs/superpowers/specs/2026-07-10-…` + `docs/superpowers/plans/2026-07-10-…` are committed with this branch (already on disk).

- [ ] **Step 1: Append ADR-0034 Amendment #13**

After Amendment #12 in `docs/adr/0034-…md`, add:

```markdown
13. **Response-body bounds — un-defers Am#6's `TimeoutBody`; wires §2's size guard.**
    The streaming mid-stream-stall `TimeoutBody` deferred in Amendment #6 lands as a
    `StallTimeoutLayer` (innermost, inside `RateLimit` so `Guarded` wraps it): a
    per-frame **inactivity** timeout via the `Timer` seam, `HttpError::Timeout` on
    stall, inert on buffered bodies and when disabled. To store the sleep future
    inline (no `Box`/`dyn`), `Timer` gains `type Sleep: Future<Output=()> + Send`.
    Independently, `BufferMode::Buffer`'s collect is **capped**
    (`ConnConfig::max_response_bytes`) via a typed `LimitedBody` wrapper plus a
    `size_hint().upper()` fast-fail, completing the max-size guard §2's
    wrapper-transparency was built to support (N1); overflow is a new non-retryable
    `HttpError::BodyTooLarge` (`ErrorKind::BodyTooLarge`, `error_kind="body_too_large"`).
    Both config values are `Option` (disable-able); `HttpConfig.body_stall_timeout`
    is `validate_config`-checked non-zero when `Some`. Cross-refs: ADR-0031 §1
    (`Timeout`), ADR-0030 §4 (buffering).
```

- [ ] **Step 2: Add the CHANGELOG entry**

In `CHANGELOG.md`, under `## [Unreleased]` → `### Added` (create the subsection if absent) and `### Changed`:

```markdown
### Added

- **net-http response-body bounds.** A streaming **stall timeout** (`StallTimeoutLayer`
  / `TimeoutBody`, `HttpConfig.body_stall_timeout`) bounds a mid-transfer body
  inactivity gap so a slow venue body can no longer wedge a concurrency permit; a
  **buffered size cap** (`LimitedBody`, `ConnConfig::max_response_bytes`) rejects an
  oversized `BufferMode::Buffer` body with the new non-retryable `HttpError::BodyTooLarge`
  (`error_kind="body_too_large"`) before OOM. Both are `Option` (disable-able).
  Implements ADR-0034 Amendment #13 (un-defers Am#6's `TimeoutBody`; wires §2's size guard).
```

Add to `### Changed` (a breaking pre-release note):

```markdown
- **Breaking (pre-release) — net timing + config surface.** `Timer` gains an associated
  `type Sleep: Future<Output=()> + Send` (was `fn sleep(&self) -> impl Future`), so body
  wrappers store the sleep future inline without boxing. `HttpConfig` gains
  `body_stall_timeout: Option<Duration>` and `ConnConfig` gains
  `max_response_bytes: Option<usize>` (both new required fields); `HttpError`/`ErrorKind`
  gain a `BodyTooLarge` variant.
```

- [ ] **Step 3: Full CI gate**

Run: `just ci`
Expected: PASS — fmt, lint, test, **doc**, deny, typos all green.

- [ ] **Step 4: Commit**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md docs/superpowers/specs/2026-07-10-net-http-response-body-bounds-design.md docs/superpowers/plans/2026-07-10-net-http-response-body-bounds.md
git commit -m "docs(net): ADR-0034 Am#13 + CHANGELOG for response-body bounds"
```

- [ ] **Step 5: Open the PR**

```bash
gh pr create --title "feat(net): response-body bounds — stall timeout + buffered size cap (ADR-0034 Am#13)" --body "Closes #<ISSUE>. Implements the Tier-2 'Streaming stall TimeoutBody' item (#102) + the untracked N1 buffer cap. See docs/superpowers/specs/2026-07-10-net-http-response-body-bounds-design.md."
```

Then tick the **Streaming stall TimeoutBody** box in issue #102.

---

## Self-Review

**Spec coverage** — every spec section maps to a task:
- §4 `Timer::Sleep` → Task 1. §7 error surface → Task 2. §5 `TimeoutBody`/`StallTimeoutLayer` → Task 3. §9 stack placement + §8 `body_stall_timeout` → Task 4. §6 `LimitedBody` → Task 5. §8 `max_response_bytes` + leaf cap → Task 6. §10 ADR + §12 CHANGELOG → Task 7. §11 tests distributed across Tasks 2–6.
- Permit-release (§9 crux) → Task 3 Step 7 (unit) + Task 4 Step 6 (full-stack stall).

**Placeholder scan** — no "TBD"/"handle errors"/"similar to". One spot to watch: Task 4 Step 6 first sketches a `Step::StreamStall` then discards it for a standalone `StallLeaf`; the executor uses `StallLeaf` (the sketch is explicitly retracted in-step).

**Type consistency** — `TimeoutBody::new(inner, Option<Duration>, timer)`, `StallTimeoutLayer::new(Option<Duration>, timer)`, `LimitedBody::new(inner, u64)`, `HttpConfig.body_stall_timeout: Option<Duration>`, `ConnConfig.max_response_bytes: Option<usize>`, `BuildError::ZeroDuration("body_stall_timeout")`, `HttpError::BodyTooLarge`, `ErrorKind::BodyTooLarge`, label `"body_too_large"` — used consistently across tasks. `LimitedBody` stores `u64`; the leaf converts `cap as usize→u64` at the call site.

**Note on defaults:** `30s` / `16 MiB` are the spec's proposals — swap in the review if IBKR data differs (config values only; no code change).
