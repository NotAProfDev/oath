//! A virtual clock for deterministically driving timing layers in tests.
//!
//! `std::time::Instant` has no value constructor, so `MockTimer` anchors to a
//! real `Instant::now()` at construction and advances via a stored offset
//! (behind interior mutability, since `Timer::now` takes `&self`). `sleep`
//! registers a waker released by `advance` — a no-op `sleep` would make
//! elapsed-time-dependent tests vacuous. Cf. `governor::clock::FakeRelativeClock`.

use oath_adapter_net_api::Timer;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct State {
    now: Instant,
    waiters: Vec<(Instant, Waker)>,
}

/// A cloneable virtual clock. Clones share one timeline.
#[derive(Debug, Clone)]
pub struct MockTimer {
    state: Arc<Mutex<State>>,
}

impl MockTimer {
    /// A clock anchored at the current real instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                now: Instant::now(),
                waiters: Vec::new(),
            })),
        }
    }

    /// Advance virtual time by `dur`, waking every sleeper now due.
    pub fn advance(&self, dur: Duration) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.now += dur;
        let now = state.now;
        let mut due = Vec::new();
        state.waiters.retain(|(deadline, waker)| {
            if *deadline <= now {
                due.push(waker.clone());
                false
            } else {
                true
            }
        });
        drop(state); // release before waking, so a woken poll can re-lock
        for waker in due {
            waker.wake();
        }
    }
}

impl Default for MockTimer {
    fn default() -> Self {
        Self::new()
    }
}

/// The future returned by [`MockTimer::sleep`].
#[derive(Debug)]
pub struct Sleep {
    state: Arc<Mutex<State>>,
    deadline: Instant,
}

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.now >= self.deadline {
            Poll::Ready(())
        } else {
            state.waiters.push((self.deadline, cx.waker().clone()));
            Poll::Pending
        }
    }
}

impl Timer for MockTimer {
    fn sleep(&self, dur: Duration) -> impl Future<Output = ()> + Send {
        let deadline = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.now + dur
        };
        Sleep {
            state: Arc::clone(&self.state),
            deadline,
        }
    }

    fn now(&self) -> Instant {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .now
    }
}

#[cfg(test)]
mod tests {
    use super::MockTimer;
    use oath_adapter_net_api::Timer;
    use std::time::Duration;

    #[tokio::test]
    async fn advance_moves_now_and_wakes_sleepers() {
        let timer = MockTimer::new();
        let start = timer.now();
        let timer_for_spawn = timer.clone();
        // Wake the sleeper by advancing past its deadline on another task.
        let handle =
            tokio::spawn(async move { timer_for_spawn.sleep(Duration::from_secs(10)).await });
        tokio::task::yield_now().await;
        timer.advance(Duration::from_secs(10));
        handle.await.unwrap();
        assert_eq!(timer.now().duration_since(start), Duration::from_secs(10));
    }
}
