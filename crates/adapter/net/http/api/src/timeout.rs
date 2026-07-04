//! The `Timeout` resilience layer (ADR-0031 §1).
//!
//! Bounds how long the inner stack may take to **produce a response** — the
//! *send*, not the pacing-permit wait (`RateLimit` sits outside it, so a
//! throttled request never enters `Timeout`). Races `inner.call(req)` against
//! [`Timer::sleep`](oath_adapter_net_api::Timer::sleep); the deadline winning
//! yields [`HttpError::Timeout`](crate::HttpError::Timeout) with the inner
//! future dropped, the inner
//! finishing first yields its `Result` verbatim. **Body-transparent:** the
//! response body is returned untouched. The per-request [`RequestTimeout`]
//! extension overrides the layer default; an absent extension uses the default.
//! Runtime-neutral: generic over [`Timer`](oath_adapter_net_api::Timer), race
//! via `futures-util`.

use std::time::Duration;

/// A per-request timeout override, carried as an `http::Request` extension.
///
/// The adapter stamps it for an endpoint that needs a non-default bound. `Copy`
/// so it survives the per-attempt request clone `Retry` performs (Slice 1). An
/// **absent** extension uses the layer default — a missing override has no
/// fail-open hazard (the global deadline still applies), so it is not rejected
/// (contrast `RateScope`, ADR-0034 Amendment #1).
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout(pub Duration);

#[cfg(test)]
mod tests {
    use super::RequestTimeout;
    use std::time::Duration;

    #[test]
    fn request_timeout_round_trips_through_request_extensions() {
        let mut req = http::Request::new(bytes::Bytes::new());
        req.extensions_mut()
            .insert(RequestTimeout(Duration::from_secs(3)));
        let got = req
            .extensions()
            .get::<RequestTimeout>()
            .copied()
            .expect("override present");
        assert_eq!(got.0, Duration::from_secs(3));
    }
}
