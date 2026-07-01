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
            return Poll::Ready(());
        }
        // Re-polls while pending (e.g. this `Sleep` in a `select!` woken by a
        // sibling future) must not stack duplicate waiters. Waiters carry no
        // per-future identity, so dedup on `(deadline, will_wake)`: skip when
        // this exact waker is already queued for this deadline — a re-poll of
        // the same future is a no-op, while an unrelated future that merely
        // shares the deadline still registers its own distinct waker.
        let already_registered = state
            .waiters
            .iter()
            .any(|(deadline, waker)| *deadline == self.deadline && waker.will_wake(cx.waker()));
        if !already_registered {
            state.waiters.push((self.deadline, cx.waker().clone()));
        }
        Poll::Pending
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
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::Duration;

    // A waker with stable Arc identity (so `will_wake` treats a clone as equal)
    // that records how often it is woken.
    struct CountingWaker(AtomicUsize);
    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn repeated_poll_does_not_stack_waiters() {
        let timer = MockTimer::new();
        let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);

        let mut sleep = pin!(timer.sleep(Duration::from_secs(1)));
        assert_eq!(sleep.as_mut().poll(&mut cx), Poll::Pending);
        // A second poll with the same waker + deadline must not re-register.
        assert_eq!(sleep.as_mut().poll(&mut cx), Poll::Pending);

        let waiters = timer.state.lock().unwrap().waiters.len();
        assert_eq!(waiters, 1, "duplicate waiter registered on re-poll");

        // Advancing past the deadline wakes the single registration exactly once.
        timer.advance(Duration::from_secs(1));
        assert_eq!(
            counter.0.load(Ordering::SeqCst),
            1,
            "sleeper woken more than once"
        );
    }

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
