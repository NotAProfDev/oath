//! [`TokioTimer`] — the tokio-backed [`Timer`] the resilience stack sleeps on.

use oath_adapter_net_api::Timer;
use std::future::Future;
use std::time::{Duration, Instant};

/// The tokio-backed [`Timer`]: [`sleep`](Timer::sleep) is `tokio::time::sleep`.
///
/// Zero-sized and `Copy` — the resilience layers hold it by value and clone it
/// across attempts at no cost.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(dur)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::TokioTimer;
    use oath_adapter_net_api::Timer;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn sleep_elapses_the_requested_duration() {
        let start = tokio::time::Instant::now();
        TokioTimer.sleep(Duration::from_secs(5)).await;
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }
}
