//! A fixed-capacity rolling window of recent breaker outcomes tracking the failure
//! rate for the error-rate trip policy (ADR-0031 Amendment #3).
//!
//! Clock-free: only host-health outcomes enter — a transport failure / `5xx`
//! ([`Outcome::Failure`]) or a reached-host `2xx`/`3xx` ([`Outcome::Success`]). A
//! `4xx`/`Auth` (`Class::Ignored`) is never pushed, and a venue `429` trips the breaker
//! immediately without a window sample. A recovered host earns a fresh window via
//! [`RateWindow::new`].
//!
//! Not yet wired into [`crate::circuit_breaker`] — that lands in the next commit on
//! this branch, which is why the `#[expect(dead_code, …)]` below exists.

// Standalone unit, landed ahead of its call site (`Breaker::record`, next commit on
// this branch) so it can be built and reviewed test-first in isolation. `expect` (not
// `allow`) makes the suppression self-clearing: once `circuit_breaker.rs` consumes the
// items in the non-test build the lint stops firing and `expect` reports itself as
// unfulfilled, forcing its own removal in Task 2. Scoped to `not(test)` because the
// unit tests below already exercise every item, so `dead_code` never fires in the test
// build — an unscoped `expect` there would be unfulfilled today.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "not yet wired into circuit_breaker.rs; removed once RateWindow/Outcome are consumed in Task 2"
    )
)]

use std::collections::VecDeque;
use std::num::NonZeroU32;

/// One breaker-relevant, host-health-bearing outcome that enters the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// `pub(crate)` in a private module: clippy's nursery `redundant_pub_crate` wants
// `pub`, but that would trip the workspace `unreachable_pub` lint instead (a `pub`
// item not reachable outside the crate). `pub(crate)` states the true visibility;
// silence the losing alternative, matching `clock.rs`/`retry_after.rs`.
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum Outcome {
    /// A transport failure (`Connection`/`Timeout`) or a `5xx` response.
    Failure,
    /// A reached-host success (`2xx`/`3xx`).
    Success,
}

/// The last-`N` outcomes as a ring, with a running failure count so the rate is O(1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::redundant_pub_crate)] // see `Outcome` above
pub(crate) struct RateWindow {
    cap: usize,
    samples: VecDeque<Outcome>,
    failures: u32,
}

impl RateWindow {
    /// An empty window of capacity `window_size`. The single backing allocation is
    /// sized once here; `push` never reallocates (it evicts before exceeding `cap`).
    pub(crate) fn new(window_size: NonZeroU32) -> Self {
        let cap = window_size.get() as usize;
        Self {
            cap,
            samples: VecDeque::with_capacity(cap),
            failures: 0,
        }
    }

    /// Record one outcome (O(1)); evict the oldest once full, keeping `failures` exact.
    pub(crate) fn push(&mut self, o: Outcome) {
        if self.samples.len() == self.cap {
            // Window full: drop the oldest. It was counted, so if it was a Failure the
            // running count is >= 1 here and the decrement cannot underflow.
            if self.samples.pop_front() == Some(Outcome::Failure) {
                self.failures -= 1;
            }
        }
        if o == Outcome::Failure {
            self.failures += 1;
        }
        self.samples.push_back(o);
    }

    /// The current live sample count.
    // `cap` (and so `samples.len()`, which never exceeds it — `push` evicts before
    // growing past `cap`) originates from `NonZeroU32::get()` in `new`, so the value
    // always fits back in a `u32`; the cast cannot truncate.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn len(&self) -> u32 {
        self.samples.len() as u32
    }

    /// Trip iff at least `min_calls` samples **and** failure rate >= `threshold_pct`.
    /// Integer cross-multiply — no float in the resilience path; `>=` trips. Widen to
    /// `u64` so `failures * 100` cannot overflow for a large `window_size`.
    pub(crate) fn should_trip(&self, min_calls: u32, threshold_pct: u32) -> bool {
        let len = self.len();
        len >= min_calls
            && u64::from(self.failures) * 100 >= u64::from(threshold_pct) * u64::from(len)
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, RateWindow};
    use std::num::NonZeroU32;

    fn win(cap: u32) -> RateWindow {
        RateWindow::new(NonZeroU32::new(cap).unwrap())
    }

    fn push_n(w: &mut RateWindow, o: Outcome, n: u32) {
        for _ in 0..n {
            w.push(o);
        }
    }

    #[test]
    fn empty_window_never_trips() {
        assert!(!win(50).should_trip(10, 50), "no samples < min_calls");
    }

    #[test]
    fn below_min_calls_never_trips_even_at_full_failure() {
        let mut w = win(50);
        push_n(&mut w, Outcome::Failure, 9); // 100% failure, but only 9 < min_calls 10
        assert!(!w.should_trip(10, 50));
    }

    #[test]
    fn all_failures_trips_exactly_at_min_calls() {
        let mut w = win(50);
        push_n(&mut w, Outcome::Failure, 9);
        assert!(!w.should_trip(10, 50), "9 samples");
        w.push(Outcome::Failure); // 10th
        assert!(w.should_trip(10, 50), "reached min_calls at 100%");
    }

    #[test]
    fn interleaved_fifty_percent_trips_at_threshold_fifty() {
        let mut w = win(50);
        for _ in 0..10 {
            w.push(Outcome::Failure);
            w.push(Outcome::Success);
        } // 10 F + 10 S = 20 samples, rate 50%
        assert!(
            w.should_trip(10, 50),
            "50% failure rate meets the 50% threshold (>= trips)"
        );
    }

    #[test]
    fn just_below_threshold_does_not_trip() {
        let mut w = win(100);
        push_n(&mut w, Outcome::Failure, 49);
        push_n(&mut w, Outcome::Success, 51); // 49/100 = 49% < 50%
        assert!(!w.should_trip(10, 50));
    }

    #[test]
    fn eviction_keeps_the_failure_count_exact() {
        let mut w = win(10);
        push_n(&mut w, Outcome::Failure, 10); // window full, all failures
        assert!(w.should_trip(10, 50));
        push_n(&mut w, Outcome::Success, 10); // evicts all 10 failures
        assert_eq!(w.len(), 10, "capacity holds");
        assert!(!w.should_trip(10, 50), "window is now all successes");
    }
}
