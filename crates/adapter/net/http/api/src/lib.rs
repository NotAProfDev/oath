//! `oath-adapter-net-http-api` — the HTTP transport contract over the kernel.
//!
//! Builds on `oath-adapter-net-api` (composition machinery + `ErrorKind` +
//! `Timer`). Defines the HTTP transport contract:
//!
//! - [`service`] — the `Service` request/reply connection shape
//! - [`error`] — `HttpError` and its `HasErrorKind` impl
//! - [`client`] — the `HttpClient` dependency-inversion seam
//! - [`body`] — `ResponseBody`, `BufferMode`, and the permit-carrying `Guarded`
//! - [`auth`] — the `AuthSource` seam, `NoAuth`, and the `Auth`/`SetHeaders` layers
//! - [`rate`] — `RateKey`, the `LimitPolicy`/`LimitDecl` vocabulary, the total
//!   `RateLimitConfig`, and the boot-time `validate_coverage` check
//! - [`rate_limit`] — the `RateLimit` layer, its `RateLimitLayer` factory, and
//!   the `RateScope` per-request directive
//! - [`retry`] — the `Retry` layer, its `RetryLayer` factory, and the
//!   `Retryable`/`RetryConfig` retry directive + schedule
//! - [`circuit_breaker`] — the `CircuitBreaker` layer, its `CircuitBreakerLayer`
//!   factory, and the `CircuitBreakerConfig` thresholds
//! - [`timeout`] — the `Timeout` layer, its `TimeoutLayer` factory, and the
//!   `RequestTimeout` per-request override
//! - [`trace`] — the `Tracing` layer and its `TracingLayer` factory (outermost;
//!   one span per request, secret-safe, routed to the ADR-0014 Telemetry plane)
//! - [`meter`] — numeric resilience metrics (breaker transitions, `Throttled`
//!   rejections, retry attempts/backoff, permit-wait) and the `RouteTemplate`
//!   cardinality seam, via the runtime-neutral `metrics` facade
//! - [`stack()`] — `HttpConfig` and the `stack()` assembly composing the canonical
//!   resilience order (ADR-0031 §1) over any leaf (Slice 2)
//!
//! The resilience layers, `stack`/`build` assembly, and backends land in later
//! slices. No async runtime, `hyper`, `reqwest`, or `serde` here.
#![forbid(unsafe_code)]

pub mod auth;
pub mod body;
pub mod circuit_breaker;
pub mod client;
mod clock;
pub mod error;
pub mod meter;
pub mod rate;
pub mod rate_limit;
pub mod retry;
pub mod service;
pub mod stack;
pub mod timeout;
pub mod trace;

pub use auth::{Auth, AuthSource, NoAuth, SetHeaders};
pub use body::{BufferMode, Guarded, ResponseBody};
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerLayer};
pub use client::HttpClient;
pub use error::{BoxError, HttpError};
pub use meter::RouteTemplate;
pub use rate::{
    BuildError, LimitDecl, LimitPolicy, RateKey, RateLimitConfig, validate_concurrency_singleton,
    validate_coverage,
};
pub use rate_limit::{RateLimit, RateLimitLayer, RateScope};
pub use retry::{Retry, RetryConfig, RetryLayer, Retryable};
pub use service::Service;
pub use stack::{HttpConfig, stack};
pub use timeout::{RequestTimeout, Timeout, TimeoutLayer};
pub use trace::{Tracing, TracingLayer};
