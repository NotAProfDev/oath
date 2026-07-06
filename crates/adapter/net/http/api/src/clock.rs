//! Saturating deadline arithmetic — `Instant + Duration` that never panics on a
//! degenerate (e.g. `Duration::MAX` "no limit" sentinel) config (L1).

use std::time::{Duration, Instant};

/// `now + dur`, saturating to a far-future instant instead of panicking when the
/// sum overflows the platform `Instant`.
///
/// Overflow means "effectively never", so saturating *forward* (not back to `now`)
/// keeps the safe direction for cooldown and permit-wait deadlines: a degenerate
/// config yields a deadline that never elapses rather than one that fires
/// immediately.
// `pub(crate)` in a private module: clippy's nursery `redundant_pub_crate` wants
// `pub`, but that would trip the workspace `unreachable_pub` lint instead (a `pub`
// item not reachable outside the crate). `pub(crate)` states the true visibility;
// silence the losing alternative, matching `hyper/src/error.rs`.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn deadline(now: Instant, dur: Duration) -> Instant {
    now.checked_add(dur).unwrap_or_else(|| {
        // The sum overflowed; ~136 years is a valid far-future stand-in on every
        // real platform. If even that overflows, fall back to `now`.
        now.checked_add(Duration::from_secs(u64::from(u32::MAX)))
            .unwrap_or(now)
    })
}

#[cfg(test)]
mod tests {
    use super::deadline;
    use std::time::{Duration, Instant};

    #[test]
    fn normal_add_matches_plain_addition() {
        let now = Instant::now();
        assert_eq!(
            deadline(now, Duration::from_secs(30)),
            now + Duration::from_secs(30)
        );
    }

    #[test]
    fn overflow_saturates_forward_instead_of_panicking() {
        let now = Instant::now();
        // `now + Duration::MAX` would panic; `deadline` must not, and must land in
        // the future so a "no limit" sentinel never elapses.
        assert!(deadline(now, Duration::MAX) > now);
    }
}
