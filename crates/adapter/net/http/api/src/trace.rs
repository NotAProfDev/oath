//! The `Tracing` resilience layer (ADR-0031 §6) — the outermost layer.
//!
//! Opens one `tracing` span per logical request and attaches it to the inner
//! future via [`Instrument`], so every event the inner
//! stack emits — including [`Retry`](crate::Retry)'s per-attempt events — nests
//! under it. The span records method, route (path only — the query is dropped),
//! status **xor** [`ErrorKind`], latency, and
//! (via `Retry`) attempt count — the ADR-0014 Telemetry plane. **Secret-safe by
//! construction:** it reads only method, path, status, `ErrorKind`, and the
//! clock — never headers, never the body. **Body-transparent:** the response is
//! returned untouched. Runtime-neutral: latency via
//! [`Timer::now`], on the zero-runtime
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
        Self {
            timer: self.timer.clone(),
        }
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
        Tracing {
            inner,
            timer: self.timer.clone(),
        }
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
        Self {
            inner: self.inner.clone(),
            timer: self.timer.clone(),
        }
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
            span.record(
                "latency_us",
                u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            );
            match &out {
                Ok(resp) => {
                    span.record("status", u64::from(resp.status().as_u16()));
                },
                Err(e) => {
                    span.record("error_kind", kind_label(e.kind()));
                },
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TracingLayer;
    use crate::{HttpError, Service};
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use http_body_util::BodyExt;
    use oath_adapter_net_api::Layer;
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
            Self {
                data: Some(Bytes::from_static(b)),
            }
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
        #[allow(clippy::manual_async_fn)]
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
        #[allow(clippy::manual_async_fn)]
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
        assert_eq!(
            store.span_fields.get("method").map(String::as_str),
            Some("GET")
        );
        assert_eq!(
            store.span_fields.get("route").map(String::as_str),
            Some("/iserver/accounts")
        );
        assert_eq!(
            store.span_fields.get("status").map(String::as_str),
            Some("200")
        );
        drop(store);
    }

    #[tokio::test]
    async fn records_error_kind_on_failure_and_omits_status() {
        let (store, _guard) = capture();
        let svc = TracingLayer::new(MockTimer::new()).layer(ErrLeaf);
        let err = svc.call(get("/x")).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_))); // returned verbatim, not swallowed
        let store = store.lock().unwrap();
        assert_eq!(
            store.span_fields.get("error_kind").map(String::as_str),
            Some("connection")
        );
        assert!(
            !store.span_fields.contains_key("status"),
            "no status on the error path"
        );
        drop(store);
    }

    #[tokio::test]
    async fn latency_reflects_the_clock_delta_exactly() {
        let (store, _guard) = capture();
        let timer = MockTimer::new();
        let svc = TracingLayer::new(timer.clone()).layer(ClockLeaf {
            timer: timer.clone(),
            elapsed: Duration::from_millis(50),
        });
        svc.call(get("/x")).await.expect("ok");
        let store = store.lock().unwrap();
        assert_eq!(
            store.span_fields.get("latency_us").map(String::as_str),
            Some("50000")
        ); // 50ms
        drop(store);
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
        assert!(
            !hay.contains("SUPERSECRET"),
            "secret leaked into telemetry:\n{hay}"
        );
        // route carries the path only — the query (with its tokens) is dropped.
        assert_eq!(
            store.span_fields.get("route").map(String::as_str),
            Some("/iserver/orders")
        );
        drop(store);
    }
}
