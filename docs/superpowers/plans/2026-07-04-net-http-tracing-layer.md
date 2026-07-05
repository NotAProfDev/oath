# net-http `Tracing` Layer (Slice 1, PR 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the outermost `Tracing<S, T>` HTTP middleware — one `tracing` span per logical request recording method, route, status, `ErrorKind`, latency, and attempt count, routed to the ADR-0014 Telemetry plane, structurally incapable of leaking secrets — plus the per-attempt instrumentation in the already-merged `Retry` layer that makes attempt count observable.

**Architecture:** A `Timer`-generic, runtime-neutral `Service` wrapper in `oath-adapter-net-http-api` on the zero-runtime `tracing` facade. `call` opens `info_span!("http.request", …)`, attaches it to the inner future via `tracing`'s `Instrument` trait (so every downstream event — including `Retry`'s per-attempt events — nests under it via context propagation), measures latency with `Timer::now()` deltas, and records `status` xor `error_kind` on completion. It reads **only** method, `uri().path()` (query dropped), status, `ErrorKind`, and the clock — never headers, never the body — so secret-safety is a property of the read surface, not a scrub. `Retry` records the final attempt count onto the *ambient current span* (`Span::current().record("attempts", …)`), a no-op when no `Tracing` span is active.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `just`, the `tracing` 0.1 facade (new runtime dep — zero executor/IO), `http`/`bytes`, `net-api::{Timer, ErrorKind, HasErrorKind, Layer}`. Tests use a capturing `tracing-subscriber` `Layer` (new dev-dep) + inline service doubles + `MockTimer` (`oath-adapter-net-mock`), driven on `tokio` (dev-only).

## Global Constraints

Every task implicitly includes these:

- **Edition 2024, MSRV 1.90.** No `unsafe` — the crate is `#![forbid(unsafe_code)]`.
- **No `unwrap`/`expect`/indexing/panic and no truncating `as` casts in non-test code** — return `Result`; use `u64::try_from(x).unwrap_or(u64::MAX)` / `u64::from(x)`, never `x as u64`. Test code is exempt for `unwrap`/`expect`/indexing.
- **`just lint` = clippy `-D warnings` + `pedantic`/`nursery`** — `#[must_use]` where asked, document all public items (`missing_docs`), `Debug` on all **public** types (`missing_debug_implementations` — hand-impl where a derive would demand `Debug`/`Clone` on `S`/`T`), `const fn` where `missing_const_for_fn` asks.
- **`just doc` per task** — `just check`/`lint`/`test` do **not** catch broken rustdoc intra-doc links; every task's verify step runs `just doc`.
- **`net-http-api` charter:** no async *runtime* — no `tokio`/`hyper`/`reqwest`/`serde` in non-dev deps. The `tracing` facade is a zero-runtime dep (no executor, no IO), so it is **allowed**; `tracing-subscriber` is **dev-only** (consistent with the existing `tokio` dev-dep).
- **net-http-api tests must NOT dev-depend on `oath-adapter-net-http-mock` (`MockClient`)** — it normal-depends on this crate, so the dev-dep closes a cycle that recompiles a second, non-unifying copy of `net-http-api`. Use **inline** service doubles + `oath-adapter-net-mock`'s `MockTimer`, exactly as `rate_limit.rs`/`retry.rs`/`body.rs` do.
- **DoD per PR:** `just ci` green (fmt, lint, test + doctests, doc, deny, typos, machete). Update `CHANGELOG.md` `[Unreleased]`. One issue → one branch → worktree → one PR (`Closes #<issue>`).

## Source spec

[docs/superpowers/specs/2026-07-04-net-http-tracing-layer-design.md](../specs/2026-07-04-net-http-tracing-layer-design.md), governed by [ADR-0031 §6](../../adr/0031-http-resilience-venue-pacing.md), [ADR-0014](../../adr/0014-observability-three-planes-deterministic-boundary.md), and [ADR-0034](../../adr/0034-http-construction-surface-auth-guarded-boot-coverage.md). This is **Slice 1, PR 5** — the outermost resilience layer, built **concurrently with the CircuitBreaker PR (PR 4)**. RateLimit (#76), Timeout (#78), Retry (#82) landed PRs 1–3.

## File Structure

- `crates/adapter/net/http/api/src/trace.rs` — **new** (Tasks 1–2). `TracingLayer<T>`, `Tracing<S, T>`, `kind_label`, the `Layer`/`Service` impls, and all tests (the capturing subscriber + inline leaves).
  - **Module is named `trace`, not `tracing`** — a module named `tracing` would shadow the `tracing` crate at the crate root. The **public types stay `Tracing`/`TracingLayer`** (re-exported), so the module name is an internal detail. (The spec says "tracing.rs"; this is the one deliberate refinement.)
- `crates/adapter/net/http/api/src/retry.rs` — **modify** (Task 2). Add per-attempt events + the ambient `attempts` record inside the existing `call` loop.
- `crates/adapter/net/http/api/src/lib.rs` — **modify** (Task 1). `pub mod trace;` + re-exports + module-doc bullet.
- `Cargo.toml` (workspace) + `crates/adapter/net/http/api/Cargo.toml` — **modify** (Task 1). Add `tracing` (dep) + `tracing-subscriber` (dev-dep).
- `docs/adr/0034-...md`, `CHANGELOG.md` — **modify** (Task 3).

Each task is one or more commits; the tasks together are one PR/issue.

---

## Setup: issue (worktree already exists)

> The isolated worktree **already exists** at `.claude/worktrees/net-http-tracing` (branch `feat/net-http-tracing`, branched off `origin/main` = #82). All tasks run inside it. Only the GitHub issue remains.

- [ ] **Create the issue**

```bash
gh issue create \
  --title "feat(net): Tracing resilience layer (Slice 1, PR 5)" \
  --label enhancement \
  --body "Slice 1 PR 5 of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-tracing-layer-design.md; ADR-0031 §6, ADR-0014).

- \`Tracing<S, T>\` + \`TracingLayer<T>\` (impl \`net-api::Layer\`): the outermost layer — one \`info\` span per logical request (method, route, status, ErrorKind, latency, attempts), attached to the inner future via \`tracing::Instrument\` so downstream events nest under it. Routed to the ADR-0014 Telemetry plane.
- Secret-safe by construction: reads only method, \`uri().path()\` (query dropped), status, \`ErrorKind\`, and the \`Timer\` clock — never headers, never the body.
- \`Retry\` gains per-attempt \`tracing\` events + an ambient \`Span::current().record(\"attempts\", n)\` (a no-op when no Tracing span is active).
- Adds the \`tracing\` facade (runtime dep, zero executor) + \`tracing-subscriber\` (dev-dep). Built concurrently with the CircuitBreaker PR (independent files)."
```

Note the issue number `#<N>` for the PR body.

---

## Task 1: `Tracing` layer — span, instrument, record, secret-safety

**Files:**
- Create: `crates/adapter/net/http/api/src/trace.rs`
- Modify: `crates/adapter/net/http/api/src/lib.rs`
- Modify: `Cargo.toml` (workspace), `crates/adapter/net/http/api/Cargo.toml`

**Interfaces:**
- Consumes: `HttpError`, `Service` (crate); `ErrorKind`, `HasErrorKind`, `Layer`, `Timer` (`oath_adapter_net_api`); the `tracing` facade.
- Produces:
  - `oath_adapter_net_http_api::TracingLayer<T>` — `impl Layer<S>` factory; `pub const fn new(timer: T) -> Self`.
  - `oath_adapter_net_http_api::Tracing<S, T>` — the wrapping `Service`; for an inner `S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync` and `T: Timer`, it is `Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError>` (body-transparent — same `B`, **no** `B: Send` bound: nothing of type `B` crosses the single await).
  - The span it opens: name `"http.request"`, fields `method`, `route`, `status`, `error_kind`, `latency_us`, `attempts` (the last four declared `Empty`, recorded later — `attempts` by Task 2's `Retry`).

- [ ] **Step 1: Add the dependencies**

In the **workspace** `Cargo.toml`, under `[workspace.dependencies]`, immediately after the existing `tracing = "0.1"` line, add:

```toml
tracing-subscriber = { version = "0.3", default-features = false, features = ["registry"] }
```

(The `registry` feature gives the span store + `LookupSpan` the capturing test layer needs; skipping `fmt`/`ansi`/`env-filter` keeps the `deny`/`machete` surface minimal. `tracing = "0.1"` is already declared and unused — this PR is its first consumer.)

In `crates/adapter/net/http/api/Cargo.toml`, add to `[dependencies]` (after `pin-project-lite = { workspace = true }`):

```toml
tracing = { workspace = true }
```

and to `[dev-dependencies]` (after `oath-adapter-net-mock = { workspace = true }`):

```toml
tracing-subscriber = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/adapter/net/http/api/src/trace.rs` with the module doc, the `use` block, the capturing subscriber, the inline leaves, and the four tests below. (The `TracingLayer`/`Tracing`/`kind_label` items land in Step 4; this compiles to a failure until then.)

```rust
//! The `Tracing` resilience layer (ADR-0031 §6) — the outermost layer.
//!
//! Opens one `tracing` span per logical request and attaches it to the inner
//! future via [`Instrument`](tracing::Instrument), so every event the inner
//! stack emits — including [`Retry`](crate::Retry)'s per-attempt events — nests
//! under it. The span records method, route (path only — the query is dropped),
//! status **xor** [`ErrorKind`](oath_adapter_net_api::ErrorKind), latency, and
//! (via `Retry`) attempt count — the ADR-0014 Telemetry plane. **Secret-safe by
//! construction:** it reads only method, path, status, `ErrorKind`, and the
//! clock — never headers, never the body. **Body-transparent:** the response is
//! returned untouched. Runtime-neutral: latency via
//! [`Timer::now`](oath_adapter_net_api::Timer::now), on the zero-runtime
//! `tracing` facade. The module is named `trace` (not `tracing`) to avoid
//! shadowing the `tracing` crate; the public types are `Tracing`/`TracingLayer`.

use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
use tracing::Instrument;
use tracing::field::Empty;

/// The stable telemetry label for an [`ErrorKind`] — a low-cardinality
/// `&'static str` for the span's `error_kind` field.
///
/// The `_` arm covers the `#[non_exhaustive]` enum, so a new variant (e.g. a
/// future `CircuitBreaker` classification added by the concurrent PR) compiles
/// without touching this layer.
const fn kind_label(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Timeout => "timeout",
        ErrorKind::Connection => "connection",
        ErrorKind::Throttled => "throttled",
        ErrorKind::Auth => "auth",
        ErrorKind::Client => "client",
        ErrorKind::Server => "server",
        _ => "unknown", // ErrorKind::Unknown and any future non_exhaustive variant
    }
}

#[cfg(test)]
mod tests {
    use super::{Tracing, TracingLayer};
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::{Layer, Timer};
    use oath_adapter_net_mock::MockTimer;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context as LayerCtx, Layer as SubLayer};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::registry::LookupSpan;

    // ---- capturing subscriber ------------------------------------------------
    // One request per test, so the single span's fields merge into one map.

    #[derive(Default)]
    struct Store {
        span_fields: BTreeMap<String, String>,
        events: Vec<BTreeMap<String, String>>,
    }
    impl Store {
        // A flat dump of every captured string — for the secret-safety scan.
        fn haystack(&self) -> String {
            let mut s = String::new();
            for (k, v) in &self.span_fields {
                s.push_str(k);
                s.push('=');
                s.push_str(v);
                s.push('\n');
            }
            for ev in &self.events {
                for (k, v) in ev {
                    s.push_str(k);
                    s.push('=');
                    s.push_str(v);
                    s.push('\n');
                }
            }
            s
        }
    }

    // Renders field values to strings. `record_str` keeps `&str` values quote-free
    // (e.g. "connection"); everything else (Display via `%`, ints, the message)
    // funnels through `record_debug`, whose Debug-of-format_args is also quote-free.
    struct StrVisit<'a>(&'a mut BTreeMap<String, String>);
    impl Visit for StrVisit<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    #[derive(Clone, Default)]
    struct Capture {
        store: Arc<Mutex<Store>>,
    }
    impl<S: Subscriber + for<'a> LookupSpan<'a>> SubLayer<S> for Capture {
        fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: LayerCtx<'_, S>) {
            let mut store = self.store.lock().unwrap();
            let mut fields = std::mem::take(&mut store.span_fields);
            attrs.record(&mut StrVisit(&mut fields));
            store.span_fields = fields;
        }
        fn on_record(&self, _id: &Id, values: &Record<'_>, _ctx: LayerCtx<'_, S>) {
            let mut store = self.store.lock().unwrap();
            let mut fields = std::mem::take(&mut store.span_fields);
            values.record(&mut StrVisit(&mut fields));
            store.span_fields = fields;
        }
        fn on_event(&self, event: &Event<'_>, _ctx: LayerCtx<'_, S>) {
            let mut fields = BTreeMap::new();
            event.record(&mut StrVisit(&mut fields));
            self.store.lock().unwrap().events.push(fields);
        }
    }

    // Install a fresh Capture as the thread-local default; return its store + the
    // RAII guard. `#[tokio::test]` is current-thread, so every `.await` below runs
    // on this thread and the `Instrument` context resolves to this subscriber.
    fn capture() -> (Arc<Mutex<Store>>, tracing::subscriber::DefaultGuard) {
        let cap = Capture::default();
        let store = cap.store.clone();
        let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap));
        (store, guard)
    }

    // ---- inline leaves (no MockClient — dev-dep cycle) -----------------------

    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(b: &'static [u8]) -> Self {
            Self { data: Some(Bytes::from_static(b)) }
        }
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

    // 200 immediately.
    #[derive(Clone)]
    struct OkLeaf;
    impl Service<http::Request<Bytes>> for OkLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async move { Ok(http::Response::new(StubBody::new(b"ok"))) }
        }
    }

    // Connection error immediately.
    #[derive(Clone)]
    struct ErrLeaf;
    impl Service<http::Request<Bytes>> for ErrLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            async move { Err(HttpError::connection("reset")) }
        }
    }

    // Advances the shared clock by `elapsed` (synchronously — MockTimer uses
    // interior mutability) before returning 200, giving the layer a deterministic
    // nonzero latency to record without spawning.
    #[derive(Clone)]
    struct ClockLeaf {
        timer: MockTimer,
        elapsed: Duration,
    }
    impl Service<http::Request<Bytes>> for ClockLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let timer = self.timer.clone();
            let elapsed = self.elapsed;
            async move {
                timer.advance(elapsed);
                Ok(http::Response::new(StubBody::new(b"ok")))
            }
        }
    }

    fn get(uri: &str) -> http::Request<Bytes> {
        http::Request::builder()
            .method("GET")
            .uri(uri)
            .body(Bytes::new())
            .unwrap()
    }

    #[tokio::test]
    async fn records_method_route_status_and_body_is_transparent() {
        let (store, _guard) = capture();
        let svc = TracingLayer::new(MockTimer::new()).layer(OkLeaf);
        let resp = svc.call(get("/iserver/accounts")).await.expect("ok");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"ok")); // Response<B> passed straight through
        let store = store.lock().unwrap();
        assert_eq!(store.span_fields.get("method").map(String::as_str), Some("GET"));
        assert_eq!(store.span_fields.get("route").map(String::as_str), Some("/iserver/accounts"));
        assert_eq!(store.span_fields.get("status").map(String::as_str), Some("200"));
    }

    #[tokio::test]
    async fn records_error_kind_on_failure_and_omits_status() {
        let (store, _guard) = capture();
        let svc = TracingLayer::new(MockTimer::new()).layer(ErrLeaf);
        let err = svc.call(get("/x")).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_))); // returned verbatim, not swallowed
        let store = store.lock().unwrap();
        assert_eq!(store.span_fields.get("error_kind").map(String::as_str), Some("connection"));
        assert!(!store.span_fields.contains_key("status"), "no status on the error path");
    }

    #[tokio::test]
    async fn latency_reflects_the_clock_delta_exactly() {
        let (store, _guard) = capture();
        let timer = MockTimer::new();
        let svc = TracingLayer::new(timer.clone())
            .layer(ClockLeaf { timer: timer.clone(), elapsed: Duration::from_millis(50) });
        svc.call(get("/x")).await.expect("ok");
        let store = store.lock().unwrap();
        assert_eq!(store.span_fields.get("latency_us").map(String::as_str), Some("50000")); // 50ms
    }

    #[tokio::test]
    async fn never_leaks_authorization_header_or_query_token() {
        let (store, _guard) = capture();
        let svc = TracingLayer::new(MockTimer::new()).layer(OkLeaf);
        let mut req = get("/iserver/orders?oauth_token=SUPERSECRET&api_key=SUPERSECRET");
        req.headers_mut().insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer SUPERSECRET"),
        );
        svc.call(req).await.expect("ok");
        let store = store.lock().unwrap();
        let hay = store.haystack();
        assert!(!hay.contains("SUPERSECRET"), "secret leaked into telemetry:\n{hay}");
        // route carries the path only — the query (with its tokens) is dropped.
        assert_eq!(store.span_fields.get("route").map(String::as_str), Some("/iserver/orders"));
    }
}
```

Also wire `lib.rs` now (so the module resolves). Add the module-doc bullet after the `timeout` bullet (the block ending at the current line 18):

```rust
//! - [`trace`] — the `Tracing` layer and its `TracingLayer` factory (outermost;
//!   one span per request, secret-safe, routed to the ADR-0014 Telemetry plane)
```

Add the module declaration after `pub mod timeout;`:

```rust
pub mod trace;
```

Add the re-export after `pub use timeout::{RequestTimeout, Timeout, TimeoutLayer};`:

```rust
pub use trace::{Tracing, TracingLayer};
```

- [ ] **Step 3: Run it to verify it fails**

Run: `just check`
Expected: FAIL — `cannot find type Tracing`/`TracingLayer` in module `trace` (only `kind_label` + tests exist).

- [ ] **Step 4: Implement the layer**

Insert between `kind_label` and the `#[cfg(test)] mod tests` in `trace.rs`:

```rust
/// The `Tracing` [`Layer`] factory: holds the [`Timer`] clock (for latency) and
/// produces a [`Tracing`] around any inner service.
pub struct TracingLayer<T> {
    timer: T,
}

impl<T> TracingLayer<T> {
    /// Build the layer with a [`Timer`] clock. Infallible — no config to check.
    #[must_use]
    pub const fn new(timer: T) -> Self {
        Self { timer }
    }
}

impl<T: Clone> Clone for TracingLayer<T> {
    fn clone(&self) -> Self {
        Self { timer: self.timer.clone() }
    }
}

impl<T> fmt::Debug for TracingLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracingLayer").finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for TracingLayer<T> {
    type Service = Tracing<S, T>;

    fn layer(&self, inner: S) -> Tracing<S, T> {
        Tracing { inner, timer: self.timer.clone() }
    }
}

/// The `Tracing` middleware: opens one span per request and records the outcome.
///
/// Body-transparent — the inner `http::Response<B>` is returned untouched.
pub struct Tracing<S, T> {
    inner: S,
    timer: T,
}

impl<S: Clone, T: Clone> Clone for Tracing<S, T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), timer: self.timer.clone() }
    }
}

impl<S, T> fmt::Debug for Tracing<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tracing").finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for Tracing<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
    // No `B: Send`: the sole await is the inner call; `record()` is synchronous,
    // so no value of type `B` ever crosses a yield point (contrast `Retry`).
{
    type Response = http::Response<B>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            // Read method + path up front — path ONLY, never the query, which can
            // carry tokens (ADR-0031 §6). `route` is owned so `req` can move on.
            let route = req.uri().path().to_owned();
            let span = tracing::info_span!(
                "http.request",
                method = %req.method(),
                route = %route,
                status = Empty,
                error_kind = Empty,
                latency_us = Empty,
                attempts = Empty,
            );
            let start = self.timer.now();
            // `.instrument` enters the span on every poll of the inner future, so
            // every downstream event (incl. Retry's per-attempt) nests under it.
            let out = self.inner.call(req).instrument(span.clone()).await;
            let elapsed = self.timer.now().saturating_duration_since(start);
            span.record("latency_us", u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
            match &out {
                Ok(resp) => {
                    span.record("status", u64::from(resp.status().as_u16()));
                }
                Err(e) => {
                    span.record("error_kind", kind_label(e.kind()));
                }
            }
            out
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `just check && cargo test -p oath-adapter-net-http-api trace && just lint && just doc`
Expected: PASS, warning-free, docs build clean.

> Known risks (from the spec's implementation notes):
> - **Module name.** `trace`, not `tracing` — a `tracing` module would shadow the crate. If you still see `info_span!`/`Instrument` failing to resolve, confirm the file is `trace.rs` and paths are the fully-qualified `tracing::…`.
> - **`tracing-subscriber` feature.** If the compiler names a missing item behind a feature (e.g. `registry`), the `registry` feature is the one that matters; add `"std"` alongside it if `Layer`/`LookupSpan` are reported missing.
> - **`S: Sync`** is required because the returned `Send` future borrows `&self`; `T: Sync` holds via `Timer`. No `B` bound.
> - **`Empty` fields** are not emitted at span creation, so `on_new_span` captures only `method`/`route`; `status`/`error_kind`/`latency_us` arrive via `on_record`. That is why the error test asserts `!contains_key("status")`.
> - If clippy's `missing_const_for_fn` rejects `const fn new` for generic `T` (it should not — construction only), drop `const`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/adapter/net/http/api/Cargo.toml \
  crates/adapter/net/http/api/src/trace.rs crates/adapter/net/http/api/src/lib.rs
git commit -m "feat(net): Tracing layer — per-request span, secret-safe, Timer latency"
```

---

## Task 2: `Retry` per-attempt instrumentation + attempt-count integration test

**Files:**
- Modify: `crates/adapter/net/http/api/src/retry.rs`
- Modify: `crates/adapter/net/http/api/src/trace.rs` (test module only)

**Interfaces:**
- Consumes: `Tracing`/`TracingLayer` (Task 1); `RetryLayer`, `RetryConfig`, `Retryable` (crate, already shipped #82); the capturing subscriber + `StubBody` (Task 1 test module).
- Produces: `Retry`'s `call` now emits a `debug` `http.attempt` event per send, a `debug` `http.retry.backoff` event per backoff, and records `attempts` onto the ambient current span — a **no-op** when no `Tracing` span (or no subscriber) is active, so `Retry`'s standalone behaviour and its existing tests are unchanged.

- [ ] **Step 1: Write the failing integration test**

Add to the `tests` module in `trace.rs`. First extend that module's imports (add these `use` lines alongside the existing ones):

```rust
    use std::sync::atomic::{AtomicUsize, Ordering};
```

Then add the scripted leaf + the test:

```rust
    // A leaf yielding one scripted status per call; repeats the last once exhausted.
    #[derive(Clone, Copy)]
    enum Step {
        Status(u16),
    }
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
    }
    impl ScriptLeaf {
        fn new(steps: Vec<Step>) -> Self {
            Self { steps: Arc::new(steps), calls: Arc::new(AtomicUsize::new(0)) }
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self.steps.get(i).copied().unwrap_or_else(|| *self.steps.last().unwrap());
            async move {
                match step {
                    Step::Status(code) => {
                        let mut resp = http::Response::new(StubBody::new(b"body"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn retry_populates_attempt_count_and_nests_per_attempt_events() {
        use crate::{RetryConfig, RetryLayer, Retryable};
        use std::num::NonZeroU32;

        let (store, _guard) = capture();
        // Zero backoff → the retry loop runs inline: MockTimer `sleep(0)` is Ready,
        // so no spawn/advance is needed to drain the backoff between attempts.
        let cfg = RetryConfig {
            max_attempts: NonZeroU32::new(3).unwrap(),
            base: Duration::ZERO,
            cap: Duration::ZERO,
            seed: 1,
        };
        let leaf = ScriptLeaf::new(vec![Step::Status(503), Step::Status(200)]);
        let svc = TracingLayer::new(MockTimer::new())
            .layer(RetryLayer::new(cfg, MockTimer::new()).layer(leaf));
        let mut req = get("/iserver/orders");
        req.extensions_mut().insert(Retryable);

        let resp = svc.call(req).await.expect("503 retried → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);

        let store = store.lock().unwrap();
        // The ambient record from inside Retry lands on the outer "http.request" span.
        assert_eq!(store.span_fields.get("attempts").map(String::as_str), Some("2"));
        // Two sends → two nested http.attempt events (message field = "http.attempt").
        let attempts = store
            .events
            .iter()
            .filter(|e| e.get("message").map(String::as_str) == Some("http.attempt"))
            .count();
        assert_eq!(attempts, 2, "one http.attempt event per send");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p oath-adapter-net-http-api retry_populates_attempt_count -- --nocapture`
Expected: FAIL — `attempts` is absent (still `Empty`) and there are zero `http.attempt` events, because `Retry` is not yet instrumented. Assertion fails on `Some("2")` / `attempts == 2`.

- [ ] **Step 3: Instrument `Retry`**

In `retry.rs`, replace the body of the `loop { … }` inside `Retry::call` (currently: fetch outcome → decide `retry` → `if !retry { return outcome; }` → drop → backoff → `attempt += 1`) with the instrumented version. The new loop body:

```rust
            loop {
                // Whole-request clone per attempt (see the existing note above this loop).
                let outcome = self.inner.call(req.clone()).await;
                // Per-attempt telemetry — nests under Tracing's span when present,
                // a no-op otherwise (ADR-0031 §6). `debug`: drill-down, pay-per-use.
                match &outcome {
                    Ok(resp) => tracing::event!(
                        tracing::Level::DEBUG,
                        attempt = u64::from(attempt),
                        status = u64::from(resp.status().as_u16()),
                        "http.attempt"
                    ),
                    Err(e) => tracing::event!(
                        tracing::Level::DEBUG,
                        attempt = u64::from(attempt),
                        error_kind = ?e.kind(),
                        "http.attempt"
                    ),
                }
                let retry = eligible
                    && attempt < max
                    && match &outcome {
                        Err(e) => is_transient(e.kind()),
                        Ok(resp) => resp.status().is_server_error(), // 5xx only; 429 is 4xx
                    };
                if !retry {
                    // Record the final attempt count onto the current span — the
                    // "http.request" span when composed under Tracing; a no-op
                    // otherwise (no active span / the field is absent).
                    tracing::Span::current().record("attempts", u64::from(attempt));
                    return outcome; // success, non-retryable outcome, or attempts exhausted
                }
                drop(outcome); // release the prior response's Guarded permit before waiting
                let ceil = backoff_ceiling(self.cfg.base, self.cfg.cap, attempt);
                let delay = self.rng.duration_in(ceil);
                tracing::event!(
                    tracing::Level::DEBUG,
                    attempt = u64::from(attempt),
                    backoff_us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX),
                    "http.retry.backoff"
                );
                self.timer.sleep(delay).await;
                attempt += 1;
            }
```

No new `use` line is needed — the events use fully-qualified `tracing::event!` / `tracing::Level` / `tracing::Span`, and `tracing` is a crate dependency as of Task 1.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p oath-adapter-net-http-api && just lint && just doc`
Expected: PASS — the new integration test passes, **and every existing `retry.rs` test still passes** (they run without a subscriber, proving the ambient record + events are a graceful no-op). Warning-free, docs clean.

> Why no separate "graceful no-op" test: the entire pre-existing `retry.rs` suite runs with no `tracing` subscriber installed, so its continued green is exactly that assertion.

- [ ] **Step 5: Commit**

```bash
git add crates/adapter/net/http/api/src/retry.rs crates/adapter/net/http/api/src/trace.rs
git commit -m "feat(net): Retry emits per-attempt tracing events + ambient attempt count"
```

---

## Task 3: ADR amendment, CHANGELOG, full gate, PR

**Files:**
- Modify: `docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: ADR-0034 append-only amendment**

Append to ADR-0034's **Amendments (2026-07-04)** numbered list, after item **8** (the Retry note), a new item **9**:

```markdown
9. **`Tracing` layer (Slice 1 PR 5).** The outermost `Tracing<S, T>` layer +
   `TracingLayer<T>` factory (ADR-0031 §6) open one `info` span per logical request
   and attach it to the inner future via `tracing::Instrument`, so downstream events
   — including `Retry`'s per-attempt events — nest under it. The span records method,
   `route` (`uri().path()` — the **query is dropped**, since it can carry tokens),
   `status` **xor** `ErrorKind` (a `_`-arm label over the `#[non_exhaustive]` enum),
   `latency_us` (via `Timer::now()` deltas — the layer is `Timer`-generic), and
   `attempts`. Routed to the ADR-0014 Telemetry plane (machinery metrics, lossy, never
   canonical). **Secret-safe by construction:** the layer reads only method, path,
   status, `ErrorKind`, and the clock — never headers, never the body. Body-transparent
   (`http::Response<B>` untouched, no `B: Send` bound — nothing of type `B` crosses the
   single await). **Composition contract:** `Tracing` owns the one per-request span;
   inner resilience layers emit `tracing` **events**, never open their own span — which
   keeps `Span::current()` at any inner depth resolved to `http.request`, so `Retry`'s
   `Span::current().record("attempts", n)` populates the field (a graceful no-op when no
   such span/field is active). Adds the `tracing` facade (runtime dep, zero executor) +
   `tracing-subscriber` (dev-dep). The module is named `trace` to avoid shadowing the
   `tracing` crate; the public types are `Tracing`/`TracingLayer`.
```

> **Numbering caveat:** the CircuitBreaker PR is in flight concurrently and also appends an amendment. If it merged first and took **#9**, renumber this one to **#10** during rebase (a mechanical fix, not a design change).

- [ ] **Step 2: CHANGELOG**

Add to `CHANGELOG.md` `[Unreleased] → Added`, at the **end of the list** (after the `Retry` resilience-layer entry):

```markdown
- `oath-adapter-net-http-api` `Tracing` resilience layer (Slice 1 PR 5) — the outermost
  `Tracing<S, T>` service + `TracingLayer<T>` factory (`net-api::Layer`): one `info` span
  per logical request (method, route, status, `ErrorKind`, latency, attempts), attached to
  the inner future via `tracing::Instrument` so downstream events — including `Retry`'s new
  per-attempt events — nest under it. Latency via `net-api::Timer` deltas; secret-safe by
  construction (reads only method, `uri().path()` with the query dropped, status,
  `ErrorKind`, and the clock — never headers or bodies); body-transparent. `Retry` now emits
  `debug` per-attempt/backoff events and records the final attempt count onto the ambient
  span (a no-op without a `Tracing` span). Routed to the ADR-0014 Telemetry plane. Adds the
  `tracing` facade (runtime dep) + `tracing-subscriber` (dev-dep). (ADR-0031 §6, ADR-0014,
  ADR-0034.)
```

- [ ] **Step 3: Full local gate**

Run: `just ci`
Expected: green — fmt, lint, test + doctests, doc, deny, typos, machete. `deny`/`machete` now see the new `tracing` (used by `trace.rs`/`retry.rs`) and `tracing-subscriber` (used by the test harness), so neither is flagged unused; `deny` must accept `tracing-subscriber`'s (small, `registry`-only) subtree.

> If `deny` rejects a license/advisory in `tracing-subscriber`'s tree, the offending crate is named in its output; the most likely case is a new transitive dep needing an allow entry in `deny.toml` — add it with a one-line justification (do not broaden the policy).

- [ ] **Step 4: Commit, push, PR**

```bash
git add docs/adr/0034-http-construction-surface-auth-guarded-boot-coverage.md CHANGELOG.md
git commit -m "docs(net): record Tracing layer amendment (ADR-0034) + changelog"
git push -u origin feat/net-http-tracing
gh pr create \
  --title "feat(net): Tracing resilience layer (Slice 1, PR 5)" \
  --body "Closes #<N>

Slice 1 **PR 5** of the net-http resilience layers (spec: docs/superpowers/specs/2026-07-04-net-http-tracing-layer-design.md; ADR-0031 §6, ADR-0014). The outermost layer; built concurrently with the CircuitBreaker PR (independent files).

- **\`Tracing<S, T>\`** + **\`TracingLayer<T>\`** (\`net-api::Layer\`) — one \`info\` span per logical request (method, route, status, \`ErrorKind\`, latency, attempts), attached to the inner future via \`tracing::Instrument\` so downstream events (incl. \`Retry\`'s per-attempt) nest under it. Latency via \`net-api::Timer\` deltas.
- **Secret-safe by construction** — reads only method, \`uri().path()\` (query dropped), status, \`ErrorKind\`, and the clock; never headers, never bodies. Body-transparent (\`Response<B>\` untouched).
- **\`Retry\` instrumentation** — \`debug\` per-attempt/backoff events + an ambient \`Span::current().record(\"attempts\", n)\` (a no-op without a \`Tracing\` span). Composition contract: \`Tracing\` owns the one span; inner layers emit events, not spans.
- Routed to the ADR-0014 **Telemetry** plane (machinery metrics, lossy, never canonical). Module named \`trace\` to avoid shadowing the \`tracing\` crate.

Adds the \`tracing\` facade (runtime dep, zero executor) + \`tracing-subscriber\` (dev-dep). \`MockTimer\`-driven tests with inline service doubles + a capturing subscriber, including the load-bearing secret-safety test.

Next: **Slice 2** — the \`stack()\`/\`build()\` assembly that wires the default layer order.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Expected: PR open, GitHub Actions CI green (same `just ci` + MSRV job).

---

## Self-Review

**Spec coverage (design doc §Scope + Decisions):**
- `Tracing<S, T>` + `TracingLayer<T>` (`Layer`), infallible `const fn new`, hand-written `Clone`/`Debug` — Task 1 Step 4. ✅
- One `info_span!("http.request", …)` with the six fields, `Empty`-then-`record` — Task 1 Steps 2/4. ✅
- `Instrument` attaches the span so downstream events nest — Task 1 Step 4 + Task 2 integration test. ✅
- Latency via `Timer::now()` deltas, `try_from`/`saturating_duration_since` (no `as`, no panic) — Task 1 Step 4 + `latency_reflects_the_clock_delta_exactly`. ✅
- Route = `uri().path()`, query dropped; secret-safety structural — Task 1 Step 4 + `never_leaks_authorization_header_or_query_token`. ✅
- Body-transparent, no `B: Send` — Task 1 `Interfaces` + Step 4 `where`-clause comment + body-transparency assertion. ✅
- `kind_label` `_`-arm over `#[non_exhaustive]` `ErrorKind` — Task 1 Step 2. ✅
- Retry per-attempt events + ambient `attempts` record; graceful no-op — Task 2 Steps 3/4. ✅
- Composition contract (inner layers emit events, not spans) — recorded in the ADR amendment (Task 3 Step 1) + relied on by the integration test. ✅
- `tracing` runtime dep + `tracing-subscriber` dev-dep (workspace + crate) — Task 1 Step 1. ✅
- Capturing-subscriber tests, `MockTimer`, inline doubles, no `MockClient` — Tasks 1–2. ✅
- ADR-0034 Amendment #10 + CHANGELOG — Task 3. ✅
- Deferred (correctly absent): `RouteLabel` templating, metric aggregation/exporters, `stack()`/`build()`, tokio `Timer`, `TimeoutBody` — noted, not built. ✅

**Placeholder scan:** none — every code step carries complete code; every command step carries an expected result. `#<N>` is the real issue number captured in Setup.

**Type consistency:**
- `TracingLayer::new(timer: T) -> Self`, `.layer(inner) -> Tracing<S, T>` — match the `Interfaces` block and every test call.
- `Tracing` `Service` impl: inner `Response = http::Response<B>` → `Response = http::Response<B>` (transparent) — matches `OkLeaf`/`ErrLeaf`/`ClockLeaf`/`ScriptLeaf` (`Response = http::Response<StubBody>`, so `B = StubBody`).
- Span field names `method`/`route`/`status`/`error_kind`/`latency_us`/`attempts` — identical in the `info_span!` (Task 1), the `record` calls (Task 1 layer + Task 2 `Retry`), and every test assertion.
- Integers recorded as `u64` throughout (`u64::from(status u16)`, `u64::try_from(micros).unwrap_or(MAX)`, `u64::from(attempt u32)`) — no `as`, lint-clean, and the capture visitor renders them quote-free.
- `RetryConfig { max_attempts, base, cap, seed }` in the Task 2 test matches the shipped `#82` struct (NonZeroU32 + two `Duration`s + `u64`), and `Retryable` is the shipped opt-in marker.
- `capture() -> (Arc<Mutex<Store>>, DefaultGuard)` — same signature used by all five tests.
- lib.rs re-export is `pub use trace::{Tracing, TracingLayer};` (module `trace`, types `Tracing`/`TracingLayer`) — consistent across File Structure, Task 1, and the public interface.

**Known risks to watch during impl:** listed inline in Task 1 Step 5 (module name, `tracing-subscriber` feature, `S: Sync`, `Empty` fields) and Task 3 Step 3 (`deny` on the new subtree).
