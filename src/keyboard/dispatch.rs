use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use super::state::{InputAction, InputEvent};
use crate::window_target::WindowTarget;

pub(crate) const WM_INPUT_READY: u32 = 6003;
const INPUT_QUEUE_CAPACITY: usize = 64;

struct PendingInput {
    events: VecDeque<ReceivedInput>,
    notified: bool,
}

pub(crate) struct ReceivedInput {
    pub(crate) event: InputEvent,
    pub(crate) received: Option<Instant>,
}

pub(crate) struct InputDispatch {
    target: Arc<WindowTarget>,
    pending: Mutex<PendingInput>,
    acknowledged: AtomicU64,
    revoked: AtomicU64,
    rejected: AtomicU64,
    callbacks: AtomicU64,
    max_callback_us: AtomicU64,
    metrics: bool,
}

impl InputDispatch {
    #[cfg(test)]
    pub(crate) fn new(target: Arc<WindowTarget>) -> Self {
        Self::with_metrics(target, true)
    }

    pub(crate) fn with_metrics(target: Arc<WindowTarget>, metrics: bool) -> Self {
        Self {
            target,
            pending: Mutex::new(PendingInput {
                events: VecDeque::with_capacity(INPUT_QUEUE_CAPACITY),
                notified: false,
            }),
            acknowledged: AtomicU64::new(0),
            revoked: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            max_callback_us: AtomicU64::new(0),
            metrics,
        }
    }

    pub(super) fn submit(&self, event: InputEvent) -> bool {
        if matches!(event.action, InputAction::Cancel) && self.permits(event.session) {
            // Cancellation already has an atomic delivery path. Do not depend
            // on queue space/ownership: rejecting Escape here would forward a
            // native modifier+Escape shortcut instead of canceling the panel.
            self.revoke(event.session);
            return true;
        }
        let accepted = self.enqueue(event, || self.target.try_post(WM_INPUT_READY));
        if !accepted {
            self.cancel(event.session);
        }
        accepted
    }

    fn enqueue(&self, event: InputEvent, notify: impl FnOnce() -> bool) -> bool {
        if !self.permits(event.session) {
            return false;
        }
        let Some(mut pending) = self.pending.try_lock() else {
            return false;
        };
        let limit = if matches!(event.action, InputAction::Cycle(..)) {
            INPUT_QUEUE_CAPACITY - 1
        } else {
            INPUT_QUEUE_CAPACITY
        };
        if pending.events.len() >= limit {
            return false;
        }
        pending.events.push_back(ReceivedInput {
            event,
            received: self.sample_start(),
        });
        if !pending.notified {
            if !notify() {
                pending.events.pop_back();
                return false;
            }
            pending.notified = true;
        }
        true
    }

    pub(crate) fn take(&self) -> Vec<InputEvent> {
        self.take_timed()
            .into_iter()
            .map(|input| input.event)
            .collect()
    }

    pub(crate) fn take_timed(&self) -> Vec<ReceivedInput> {
        let mut pending = self.pending.lock();
        pending.notified = false;
        pending.events.drain(..).collect()
    }

    pub(crate) fn cancel(&self, session: u64) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
        self.revoke(session);
    }

    fn revoke(&self, session: u64) {
        self.revoked.fetch_max(session, Ordering::AcqRel);
        // A UI timer also polls revocation, so a failed PostMessage cannot lose
        // a terminal cancellation. Nothing here waits for the UI or its queue.
        self.target.try_post(WM_INPUT_READY);
    }

    pub(crate) fn acknowledge(&self, session: u64) {
        self.acknowledged.fetch_max(session, Ordering::Release);
    }
    pub(super) fn acknowledged(&self) -> u64 {
        self.acknowledged.load(Ordering::Acquire)
    }
    pub(crate) fn revoked(&self) -> u64 {
        self.revoked.load(Ordering::Acquire)
    }
    pub(crate) fn permits(&self, session: u64) -> bool {
        self.target.is_live() && session > self.revoked() && session > self.acknowledged()
    }
    pub(super) fn is_live(&self) -> bool {
        self.target.is_live()
    }

    pub(super) fn observe_callback(&self, elapsed: Duration) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.max_callback_us.fetch_max(
            elapsed.as_micros().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub(super) fn sample_start(&self) -> Option<Instant> {
        self.metrics.then(Instant::now)
    }

    pub(crate) fn log_summary(&self) {
        if !self.metrics {
            return;
        }
        info!(
            "input stage=summary callbacks={} rejected={} max_callback_us={}",
            self.callbacks.load(Ordering::Relaxed),
            self.rejected.load(Ordering::Relaxed),
            self.max_callback_us.load(Ordering::Relaxed)
        );
    }

    #[cfg(test)]
    pub(super) fn callback_statistics(&self) -> (u64, u64) {
        (
            self.callbacks.load(Ordering::Relaxed),
            self.max_callback_us.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::SwitchKind;
    use super::*;
    use windows::Win32::Foundation::HWND;
    fn queue() -> InputDispatch {
        InputDispatch::new(Arc::new(WindowTarget::new(HWND::default())))
    }
    fn cycle(session: u64) -> InputEvent {
        InputEvent {
            session,
            action: InputAction::Cycle(SwitchKind::Apps, false),
        }
    }

    #[test]
    fn bounded_queue_reserves_a_terminal_slot_and_requires_ack() {
        let queue = queue();
        for _ in 0..INPUT_QUEUE_CAPACITY - 1 {
            assert!(queue.enqueue(cycle(1), || true));
        }
        assert!(!queue.enqueue(cycle(1), || true));
        assert!(queue.enqueue(
            InputEvent {
                session: 1,
                action: InputAction::Finish(SwitchKind::Apps)
            },
            || true
        ));
        assert_eq!(queue.acknowledged(), 0);
        assert_eq!(queue.take().len(), INPUT_QUEUE_CAPACITY);
        queue.acknowledge(1);
        assert_eq!(queue.acknowledged(), 1);
    }

    #[test]
    fn contention_notification_failure_and_retirement_fail_open() {
        let queue = queue();
        let pending = queue.pending.lock();
        assert!(!queue.enqueue(cycle(1), || true));
        drop(pending);
        assert!(!queue.enqueue(cycle(1), || false));
        assert!(queue.take().is_empty());
        queue.revoked.store(1, Ordering::Release);
        assert!(!queue.enqueue(cycle(1), || true));
        queue.target.close();
        assert!(!queue.enqueue(cycle(2), || true));
    }

    #[test]
    fn explicit_cancel_is_accepted_without_waiting_for_the_input_queue() {
        let queue = Arc::new(queue());
        assert!(queue.enqueue(cycle(1), || true));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let held_queue = queue.clone();
        let holder = std::thread::spawn(move || {
            let _pending = held_queue.pending.lock();
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let accepted = queue.submit(InputEvent {
            session: 1,
            action: InputAction::Cancel,
        });
        let elapsed = started.elapsed();
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(accepted, "Escape must stay consumed when the queue is busy");
        assert!(elapsed < Duration::from_millis(500));
        assert_eq!(queue.revoked(), 1);
        assert_eq!(queue.rejected.load(Ordering::Relaxed), 0);
        assert!(queue
            .take()
            .iter()
            .all(|event| !queue.permits(event.session)));
    }

    #[test]
    fn explicit_cancel_survives_failed_notification_but_not_retirement() {
        // HWND_BOTTOM is a positioning sentinel, never a message endpoint.
        let queue = InputDispatch::new(Arc::new(WindowTarget::new(HWND(1 as _))));
        assert!(!queue.target.try_post(WM_INPUT_READY));
        assert!(queue.submit(InputEvent {
            session: 1,
            action: InputAction::Cancel,
        }));
        assert!(!queue.permits(1));
        queue.target.close();
        assert!(!queue.submit(InputEvent {
            session: 2,
            action: InputAction::Cancel,
        }));
    }

    #[test]
    fn activation_rechecks_revocation_acknowledgement_and_retirement() {
        let queue = queue();
        assert!(!queue.permits(0));
        assert!(queue.permits(1));
        queue.cancel(1);
        assert!(
            !queue.permits(1),
            "a slow query must not activate after cancellation"
        );
        assert!(queue.permits(2));
        queue.acknowledge(2);
        assert!(
            !queue.permits(2),
            "a clicked session must ignore its queued finish"
        );
        assert!(!queue.enqueue(cycle(2), || true));
        assert!(queue.permits(3));
        queue.target.close();
        assert!(!queue.permits(3));
    }
}
