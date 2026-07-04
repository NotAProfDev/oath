//! The `RateLimit` resilience layer (ADR-0031 §3).
//!
//! Proactive per-endpoint pacing built from a validated
//! [`RateLimitConfig`](crate::RateLimitConfig), plus the per-request
//! [`RateScope`] directive that selects which buckets a request spends
//! against. Runtime-neutral: generic over
//! [`Timer`](oath_adapter_net_api::Timer), semaphore via `async-lock`.

/// Which bucket sets a request spends against (ADR-0031 §3). Stamped by the
/// adapter as part of a [`RateScope`] request extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    /// Acquire nothing — the **explicit** unlimited opt-out.
    None,
    /// Spend against the account-wide global bucket only.
    Global,
    /// Spend against this endpoint's local bucket only.
    Local,
    /// Spend against both the global and the local bucket.
    Both,
}

/// The per-request pacing directive, carried as an `http::Request` extension.
///
/// The adapter stamps it when it builds each request (it knows the endpoint).
/// An **absent** directive defaults to [`Scope::Global`] — you cannot bypass the
/// account-wide budget by forgetting to stamp. `Clone` so it survives the
/// per-attempt request clone `Retry` performs (Slice 1).
#[derive(Debug, Clone)]
pub struct RateScope<K> {
    /// Which bucket sets to spend against.
    pub scope: Scope,
    /// The endpoint key, required for `Local`/`Both`.
    pub key: Option<K>,
}

#[cfg(test)]
mod tests {
    use super::{RateScope, Scope};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum TestKey {
        History,
    }
    impl crate::RateKey for TestKey {
        fn all() -> &'static [Self] {
            &[Self::History]
        }
    }

    #[test]
    fn rate_scope_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut().insert(RateScope {
            scope: Scope::Both,
            key: Some(TestKey::History),
        });
        let got = req
            .extensions()
            .get::<RateScope<TestKey>>()
            .cloned()
            .expect("directive present");
        assert!(matches!(got.scope, Scope::Both));
        assert_eq!(got.key, Some(TestKey::History));
    }
}
