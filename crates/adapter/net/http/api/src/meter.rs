//! Numeric resilience metrics for the ADR-0014 Telemetry plane.
//!
//! Thin wrappers over the runtime-neutral [`metrics`] facade: the resilience
//! layers emit counters/histograms here, and the **downstream binary** installs the
//! recorder/exporter (Prometheus/OTel/…). With no recorder installed every emit is a
//! cheap no-op, so this keeps `net-http-api` runtime-neutral (ADR-0029) while making
//! the signals this stack exists to manage — breaker transitions, `Throttled`
//! rejections, retry amplification, permit-wait — dashboardable and alertable.
//!
//! **Cardinality-safe:** the only per-request label is [`RouteTemplate`], a
//! low-cardinality route the adapter stamps; an unstamped request labels as
//! `"other"`, never the raw (ID-bearing) path.

use bytes::Bytes;
use std::borrow::Cow;
use std::time::Duration;

/// A low-cardinality route label the adapter stamps as an `http::Request` extension.
///
/// Templating ID-bearing paths (e.g. `/iserver/account/{acctId}/order/{orderId}`)
/// keeps metric/label cardinality bounded (deep review §2C).
///
/// Stamp the **template**, not the concrete path: `"/iserver/account/{id}/order/{id}"`,
/// never `"/iserver/account/DU123/order/456"`. Absent ⇒ telemetry labels the request
/// `"other"`.
#[derive(Debug, Clone)]
pub struct RouteTemplate(pub Cow<'static, str>);

impl RouteTemplate {
    /// A template from a `'static` string — the common case (route templates are
    /// compile-time constants).
    #[must_use]
    pub const fn from_static(template: &'static str) -> Self {
        Self(Cow::Borrowed(template))
    }
}

/// Counter: circuit-breaker phase transitions, labelled by the entered phase
/// (`to = "open" | "half_open" | "closed"`). The C1 lockout shows up here.
const CIRCUIT_TRANSITIONS: &str = "http_circuit_breaker_transitions_total";
/// Counter: local pacing rejections (`HttpError::Throttled`), labelled by `route`.
const THROTTLED: &str = "http_rate_limit_throttled_total";
/// Counter: retry attempts that may be followed by another, labelled by `route`.
const RETRY_ATTEMPTS: &str = "http_retry_attempts_total";
/// Histogram: seconds spent waiting for pacing admission before a send, by `route`.
const PERMIT_WAIT_SECONDS: &str = "http_rate_limit_permit_wait_seconds";
/// Histogram: retry backoff delay in seconds, by `route`.
const BACKOFF_SECONDS: &str = "http_retry_backoff_seconds";
/// Counter: honored `Retry-After` directives, labelled by `site` (`"retry" | "breaker"`).
const RETRY_AFTER_HONORED: &str = "http_retry_after_honored_total";

/// The bounded `route` label for one request: the stamped [`RouteTemplate`], or the
/// fixed `"other"` when absent — never the raw path.
pub(crate) fn route_label(req: &http::Request<Bytes>) -> Cow<'static, str> {
    req.extensions()
        .get::<RouteTemplate>()
        .map_or(Cow::Borrowed("other"), |t| t.0.clone())
}

/// Record a circuit-breaker phase transition into `to`.
pub(crate) fn breaker_transition(to: &'static str) {
    metrics::counter!(CIRCUIT_TRANSITIONS, "to" => to).increment(1);
}

/// Count one local pacing rejection for `route`.
pub(crate) fn throttled(route: Cow<'static, str>) {
    metrics::counter!(THROTTLED, "route" => route).increment(1);
}

/// Count one retry attempt (a send that another attempt may follow) for `route`.
pub(crate) fn retry_attempt(route: Cow<'static, str>) {
    metrics::counter!(RETRY_ATTEMPTS, "route" => route).increment(1);
}

/// Record how long pacing admission waited before the send, for `route`.
pub(crate) fn permit_wait(route: Cow<'static, str>, waited: Duration) {
    metrics::histogram!(PERMIT_WAIT_SECONDS, "route" => route).record(waited.as_secs_f64());
}

/// Record a retry backoff delay for `route`.
pub(crate) fn backoff(route: Cow<'static, str>, delay: Duration) {
    metrics::histogram!(BACKOFF_SECONDS, "route" => route).record(delay.as_secs_f64());
}

/// Count one honored `Retry-After` directive at `site` (`"retry"` or `"breaker"`).
pub(crate) fn retry_after_honored(site: &'static str) {
    metrics::counter!(RETRY_AFTER_HONORED, "site" => site).increment(1);
}

#[cfg(test)]
mod tests {
    use super::{RouteTemplate, breaker_transition, retry_after_honored, route_label, throttled};
    use bytes::Bytes;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    #[test]
    fn route_label_is_the_template_or_other() {
        let mut req = http::Request::new(Bytes::new());
        assert_eq!(
            route_label(&req).as_ref(),
            "other",
            "absent ⇒ bounded default"
        );
        req.extensions_mut().insert(RouteTemplate::from_static(
            "/iserver/account/{id}/order/{id}",
        ));
        assert_eq!(
            route_label(&req).as_ref(),
            "/iserver/account/{id}/order/{id}"
        );
    }

    #[test]
    fn breaker_transition_increments_a_phase_labelled_counter() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            breaker_transition("open");
            breaker_transition("open");
        });
        let counter = snap
            .snapshot()
            .into_vec()
            .into_iter()
            .find(|(k, _, _, _)| k.key().name() == "http_circuit_breaker_transitions_total")
            .expect("counter emitted");
        assert!(
            counter
                .0
                .key()
                .labels()
                .any(|l| l.key() == "to" && l.value() == "open"),
            "labelled to=open"
        );
        assert_eq!(counter.3, DebugValue::Counter(2), "two transitions counted");
    }

    #[test]
    fn throttled_counter_carries_the_route_label() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            throttled(std::borrow::Cow::Borrowed("/iserver/marketdata/history"));
        });
        let counter = snap
            .snapshot()
            .into_vec()
            .into_iter()
            .find(|(k, _, _, _)| k.key().name() == "http_rate_limit_throttled_total")
            .expect("counter emitted");
        assert!(
            counter
                .0
                .key()
                .labels()
                .any(|l| l.key() == "route" && l.value() == "/iserver/marketdata/history"),
        );
        assert_eq!(counter.3, DebugValue::Counter(1));
    }

    #[test]
    fn retry_after_honored_carries_the_site_label() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            retry_after_honored("breaker");
        });
        let counter = snap
            .snapshot()
            .into_vec()
            .into_iter()
            .find(|(k, _, _, _)| k.key().name() == "http_retry_after_honored_total")
            .expect("counter emitted");
        assert!(
            counter
                .0
                .key()
                .labels()
                .any(|l| l.key() == "site" && l.value() == "breaker"),
            "labelled site=breaker"
        );
        assert_eq!(counter.3, DebugValue::Counter(1));
    }
}
