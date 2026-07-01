//! The `Timer` clock contract — a runtime-neutral clock for timing layers.

use std::future::Future;
use std::time::{Duration, Instant};

/// A clock abstraction for timing layers, decoupled from any async runtime.
///
/// Timing middleware (`Timeout`, `Retry` backoff, `RateLimit` refill,
/// `CircuitBreaker` cooldown) is generic over `Timer` so a mock clock can drive
/// it deterministically in tests while production passes a runtime-backed impl.
/// A trait — not a runtime — so the kernel stays std-only (ADR-0029 §4).
pub trait Timer: Clone + Send + Sync {
    /// Complete after `dur` has elapsed.
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send;

    /// The current instant — for elapsed-time reads (token-bucket refill,
    /// circuit cooldown).
    fn now(&self) -> Instant;
}

#[cfg(test)]
mod tests {
    use super::Timer;
    use std::future::Future;
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct FixedTimer(Instant);

    impl Timer for FixedTimer {
        fn sleep(&self, _dur: Duration) -> impl Future<Output = ()> + Send {
            std::future::ready(())
        }
        fn now(&self) -> Instant {
            self.0
        }
    }

    #[test]
    fn now_returns_the_configured_instant() {
        let t0 = Instant::now();
        assert_eq!(FixedTimer(t0).now(), t0);
    }
}
