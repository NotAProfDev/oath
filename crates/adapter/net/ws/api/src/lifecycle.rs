//! The out-of-band lifecycle channel: connection health as a third handle,
//! not an item interleaved into the data stream (ADR-0032 §4).
//!
//! Delivery is a **last-value watch of an epoch-stamped snapshot** (ADR-0033
//! §5): the producer overwrites and never blocks, so a slow risk consumer can
//! never backpressure the socket-owning actor; the monotonic `epoch` makes
//! coalescing lossless for the safety fact ({ currently-down ∨ epoch-advanced }
//! is total). Every snapshot field is **level or monotonic-cumulative, never a
//! per-event delta** — overwrite semantics would lose a delta.

use std::fmt;
use std::time::Instant;

/// The connection phase — the level component of [`LifecycleSnapshot`].
///
/// `Stale`/`Reconnecting` are first-class: for a trading system the
/// safety-critical event is the feed going *down*, not its recovery
/// (ADR-0032 §4). `Unrecoverable` is the one terminal phase (ADR-0033 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnState {
    /// The connection is up. `epoch` echoes [`LifecycleSnapshot::epoch`].
    Connected {
        /// The connection epoch at the time of this phase.
        epoch: u64,
    },
    /// The feed is stale (idle-read timeout / missed liveness) — treat as down.
    Stale,
    /// The connection is down; the reconnect actor is re-establishing it.
    Reconnecting,
    /// A reconnect completed; `epoch` bounds the adapter's reconcile window
    /// ("reconcile since epoch N", ADR-0032 §4/§5).
    Resumed {
        /// The new connection epoch after the completed down-cycle.
        epoch: u64,
    },
    /// A classified permanent failure — the stack has stopped retrying and
    /// will not self-heal (ADR-0033 §7).
    Unrecoverable,
}

/// One epoch-stamped, level-semantics view of connection health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    /// The current connection phase (level).
    pub phase: ConnState,
    /// Monotonic; bumped on every completed down-cycle. The canonical source
    /// of truth — the value echoed in `Connected`/`Resumed` is this field;
    /// consumers diff it.
    pub epoch: u64,
    /// When the current down phase began (`None` while up).
    pub down_since: Option<Instant>,
    /// Monotonic count of connection attempts.
    pub attempts: u64,
    /// Monotonic cumulative count of frames dropped by the buffer layer — the
    /// `Lagged` signal (ADR-0032 §6); a per-event delta would be lost to
    /// overwrite, so consumers diff this total.
    pub total_lagged: u64,
}

impl LifecycleSnapshot {
    /// A healthy just-connected snapshot at `epoch`.
    #[must_use]
    pub const fn connected(epoch: u64) -> Self {
        Self {
            phase: ConnState::Connected { epoch },
            epoch,
            down_since: None,
            attempts: 0,
            total_lagged: 0,
        }
    }
}

/// The read side of the lifecycle channel — the third handle `connect` yields.
///
/// Cloneable: risk loop and adapter each hold their own cursor.
#[derive(Clone)]
pub struct Lifecycle {
    rx: async_watch::Receiver<LifecycleSnapshot>,
}

impl Lifecycle {
    /// Create a linked producer/consumer pair seeded with `initial`.
    #[must_use]
    pub fn channel(initial: LifecycleSnapshot) -> (LifecycleSender, Self) {
        let (tx, rx) = async_watch::channel(initial);
        (LifecycleSender { tx }, Self { rx })
    }

    /// The latest snapshot — a level read; never blocks.
    #[must_use]
    pub fn snapshot(&self) -> LifecycleSnapshot {
        *self.rx.borrow()
    }

    /// Wait until a snapshot newer than the last one seen by *this* handle is
    /// published. Returns `false` once the producer is dropped (terminal — no
    /// further updates will ever arrive).
    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }
}

impl fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycle")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

/// The write side, held by the connection owner (a leaf, the reconnect actor,
/// or a mock). `send` overwrites — it never blocks on a slow consumer.
pub struct LifecycleSender {
    tx: async_watch::Sender<LifecycleSnapshot>,
}

impl LifecycleSender {
    /// Publish a new snapshot. Best-effort: sending with every receiver gone
    /// is a no-op, not a failure — the producer never depends on consumers.
    pub fn send(&self, snapshot: LifecycleSnapshot) {
        // Err means all receivers dropped — nothing to notify.
        let _ = self.tx.send(snapshot);
    }
}

impl fmt::Debug for LifecycleSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifecycleSender").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnState, Lifecycle, LifecycleSnapshot};

    #[test]
    fn connected_seeds_a_consistent_snapshot() {
        let snap = LifecycleSnapshot::connected(3);
        assert_eq!(snap.phase, ConnState::Connected { epoch: 3 });
        assert_eq!(snap.epoch, 3);
        assert_eq!(snap.down_since, None);
        assert_eq!(snap.attempts, 0);
        assert_eq!(snap.total_lagged, 0);
    }

    #[test]
    fn snapshot_reads_the_latest_value_without_blocking() {
        let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
        assert_eq!(
            lifecycle.snapshot().phase,
            ConnState::Connected { epoch: 0 }
        );
        tx.send(LifecycleSnapshot {
            phase: ConnState::Stale,
            ..LifecycleSnapshot::connected(0)
        });
        assert_eq!(lifecycle.snapshot().phase, ConnState::Stale);
    }

    #[test]
    fn overwrite_coalesces_but_the_epoch_carries_the_cycle_count() {
        // ADR-0033 §5: a slow consumer may miss transient phases, but
        // { currently-down ∨ epoch-advanced } is total — the epoch delta
        // recovers fully-coalesced down-cycles.
        let (tx, lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(5));
        tx.send(LifecycleSnapshot {
            phase: ConnState::Reconnecting,
            ..LifecycleSnapshot::connected(5)
        });
        tx.send(LifecycleSnapshot::connected(9)); // several cycles later
        let seen = lifecycle.snapshot();
        assert_eq!(seen.phase, ConnState::Connected { epoch: 9 });
        assert_eq!(seen.epoch - 5, 4); // four down-cycles, regardless of what was witnessed
    }

    #[tokio::test]
    async fn changed_wakes_on_update_and_ends_when_the_sender_drops() {
        let (tx, mut lifecycle) = Lifecycle::channel(LifecycleSnapshot::connected(0));
        tx.send(LifecycleSnapshot {
            phase: ConnState::Reconnecting,
            ..LifecycleSnapshot::connected(0)
        });
        assert!(lifecycle.changed().await); // pending update is observed
        assert_eq!(lifecycle.snapshot().phase, ConnState::Reconnecting);
        drop(tx);
        assert!(!lifecycle.changed().await); // producer gone — no more updates
    }
}
