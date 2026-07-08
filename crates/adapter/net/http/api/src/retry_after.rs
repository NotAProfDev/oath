//! Parse the `Retry-After` response header (RFC 9110 §10.2.3), `delay-seconds` form.
//!
//! `Retry-After` rides on `429`/`503` responses. The `Retry` and `CircuitBreaker`
//! layers read it (read-only, ADR-0034 §4) to pace by the venue's directive instead
//! of a purely local schedule (ADR-0031 Amendment #2). Only the `delay-seconds` form
//! is honored; an `HTTP-date`, a float, an overflowing integer, or an absent header
//! yields `None`, and the caller falls back to its own default — `Retry-After` is an
//! untrusted hint, so parsing never errors and never panics.

use std::time::Duration;

/// The venue-directed wait from a `Retry-After` header, `delay-seconds` form only.
///
/// Returns `None` for an absent header, a non-ASCII value, or any non-integer form
/// (an `HTTP-date`, a float such as `1.5`, a negative, or junk) — the caller falls
/// back to its own default. Never panics: `Duration::from_secs` is total over `u64`,
/// and an out-of-`u64` value simply fails to parse. The value is **uncapped** — each
/// caller clamps to its own ceiling.
// `pub(crate)` in a private module: clippy's nursery `redundant_pub_crate` wants
// `pub`, but that would trip the workspace `unreachable_pub` lint instead (a `pub`
// item not reachable outside the crate). `pub(crate)` states the true visibility;
// silence the losing alternative, matching `clock.rs`.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn parse_retry_after(headers: &http::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::parse_retry_after;
    use std::time::Duration;

    fn headers_with(value: &str) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        h.insert(
            http::header::RETRY_AFTER,
            http::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn parses_the_delay_seconds_form() {
        assert_eq!(
            parse_retry_after(&headers_with("120")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after(&headers_with("0")),
            Some(Duration::ZERO),
            "0 is a valid 'retry now'"
        );
        assert_eq!(
            parse_retry_after(&headers_with("  120  ")),
            Some(Duration::from_secs(120)),
            "surrounding whitespace is trimmed"
        );
        assert_eq!(
            parse_retry_after(&headers_with("259200")),
            Some(Duration::from_secs(259_200)),
            "a large valid integer parses; the CALLER caps it, not the parser"
        );
    }

    #[test]
    fn an_absent_header_is_none() {
        assert_eq!(parse_retry_after(&http::HeaderMap::new()), None);
    }

    #[test]
    fn non_integer_forms_fall_back_to_none() {
        // The HTTP-date form (deferred — needs a wall-clock Timer seam), a float, a
        // negative, and junk all yield None so the caller keeps its own schedule.
        assert_eq!(
            parse_retry_after(&headers_with("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(&headers_with("1.5")), None);
        assert_eq!(parse_retry_after(&headers_with("-5")), None);
        assert_eq!(parse_retry_after(&headers_with("soon")), None);
    }

    #[test]
    fn an_overflowing_integer_is_none_not_a_panic() {
        // u64::MAX + 1 — must not panic, just fail to parse (the no-panic guarantee).
        assert_eq!(
            parse_retry_after(&headers_with("18446744073709551616")),
            None
        );
    }
}
