//! The hyper backend for the OATH HTTP stack: a pooled, TLS-terminating leaf and
//! the `build()` construction surface (ADR-0030 §7).
//!
//! This is the only crate that depends on `hyper`/`tokio`/`rustls`. `build()`
//! assembles the canonical resilience stack (`oath_adapter_net_http_api::stack`)
//! over a fresh `hyper_leaf`, so backend choice stays behind the `HttpClient`
//! seam (ADR-0030 §6).

pub mod timer;

pub use timer::TokioTimer;
