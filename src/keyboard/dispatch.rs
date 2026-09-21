use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use super::state::{InputAction, InputEvent, InputSurface};
use crate::window_target::WindowTarget;

pub(crate) const WM_INPUT_READY: u32 = 6003;
const INPUT_QUEUE_CAPACITY: usize = 64;

pub(crate) struct ReceivedInput {
    pub(crate) event: InputEvent,
    pub(crate) received: Option<Instant>,
}

pub(crate) struct InputDispatch {
    target: Arc<WindowTarget>,
    sender: SyncSender<ReceivedInput>,
    pending: Mutex<Receiver<ReceivedInput>>,
    queued: AtomicUsize,
    notified: AtomicBool,
    acknowledged: AtomicU64,
    revoked: AtomicU64,
    paused: AtomicBool,
    panel_window: AtomicUsize,
    details_window: AtomicUsize,
    pause_requested: AtomicBool,
    rejected: AtomicU64,
    inactive_rejections: AtomicU64,
    queue_full_rejections: AtomicU64,
    disconnected_rejections: AtomicU64,
    notification_deferrals: AtomicU64,
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
        let (sender, receiver) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        Self {
            target,
            sender,
            pending: Mutex::new(receiver),
            queued: AtomicUsize::new(0),
            notified: AtomicBool::new(false),
            acknowledged: AtomicU64::new(0),
            revoked: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            panel_window: AtomicUsize::new(0),
            details_window: AtomicUsize::new(0),
            pause_requested: AtomicBool::new(false),
            rejected: AtomicU64::new(0),
            inactive_rejections: AtomicU64::new(0),
            queue_full_rejections: AtomicU64::new(0),
            disconnected_rejections: AtomicU64::new(0),
            notification_deferrals: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            max_callback_us: AtomicU64::new(0),
            metrics,
        }
    }

    pub(super) fn submit(&self, event: InputEvent) -> bool {
        if event.action == InputAction::TogglePause {
            if !self.target.is_live() {
                return false;
            }
            self.pause_requested.store(true, Ordering::Release);
            self.target.try_post(WM_INPUT_READY);
            return true;
        }
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
            self.inactive_rejections.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let limit = if matches!(event.action, InputAction::Cycle(..)) {
            INPUT_QUEUE_CAPACITY - 1
        } else {
            INPUT_QUEUE_CAPACITY
        };
        // Reserve before publication so the receiver can never release an
        // uncounted event. Ordinary cycles leave the final slot for a terminal.
        if self
            .queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .is_err()
        {
            self.queue_full_rejections.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if let Err(error) = self.sender.try_send(ReceivedInput {
            event,
            received: self.sample_start(),
        }) {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            let counter = match error {
                TrySendError::Full(_) => &self.queue_full_rejections,
                TrySendError::Disconnected(_) => &self.disconnected_rejections,
            };
            counter.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if !self.notified.swap(true, Ordering::AcqRel) && !notify() {
            self.notified.store(false, Ordering::Release);
            if !self.target.is_live() {
                self.inactive_rejections.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            // Notification is only a wake-up hint. The UI timer also drains
            // this bounded queue, including a finish queued after a cycle.
            self.notification_deferrals.fetch_add(1, Ordering::Relaxed);
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
        let Some(pending) = self.pending.try_lock() else {
            return Vec::new();
        };
        self.notified.store(false, Ordering::Release);
        let mut events = Vec::with_capacity(self.queued.load(Ordering::Acquire));
        // A producer may keep sending while the UI consumes. Bound each turn
        // as well as the channel, so input cannot starve painting or shutdown.
        for _ in 0..INPUT_QUEUE_CAPACITY {
            let Ok(input) = pending.try_recv() else {
                break;
            };
            self.queued.fetch_sub(1, Ordering::AcqRel);
            events.push(input);
        }
        events
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
        !self.paused()
            && self.target.is_live()
            && session > self.revoked()
            && session > self.acknowledged()
    }
    pub(crate) fn paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }
    pub(crate) fn set_surface(
        &self,
        surface: InputSurface,
        hwnd: windows::Win32::Foundation::HWND,
    ) {
        self.panel_window.store(0, Ordering::Release);
        self.details_window.store(0, Ordering::Release);
        match surface {
            InputSurface::Panel => self.panel_window.store(hwnd.0 as usize, Ordering::Release),
            InputSurface::Details => self
                .details_window
                .store(hwnd.0 as usize, Ordering::Release),
            InputSurface::None => {}
        }
    }
    pub(super) fn surface(&self) -> InputSurface {
        let foreground = crate::utils::get_foreground_window().0 as usize;
        if foreground != 0 && foreground == self.panel_window.load(Ordering::Acquire) {
            InputSurface::Panel
        } else if foreground != 0 && foreground == self.details_window.load(Ordering::Acquire) {
            InputSurface::Details
        } else {
            InputSurface::None
        }
    }
    pub(crate) fn take_pause_request(&self) -> bool {
        self.pause_requested.swap(false, Ordering::AcqRel) && self.target.is_live()
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
            "input stage=summary callbacks={} rejected={} max_callback_us={} inactive={} queue_full={} disconnected={} notify_deferred={}",
            self.callbacks.load(Ordering::Relaxed),
            self.rejected.load(Ordering::Relaxed),
            self.max_callback_us.load(Ordering::Relaxed),
            self.inactive_rejections.load(Ordering::Relaxed),
            self.queue_full_rejections.load(Ordering::Relaxed),
            self.disconnected_rejections.load(Ordering::Relaxed),
            self.notification_deferrals.load(Ordering::Relaxed)
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
    fn recovery_has_a_separate_slot_while_paused_or_queue_contended() {
        let queue = queue();
        queue.set_paused(true);
        assert!(!queue.permits(1));
        let _held = queue.pending.lock();
        let recovery = InputEvent {
            session: 0,
            action: InputAction::TogglePause,
        };
        assert!(queue.submit(recovery));
        assert!(queue.submit(recovery));
        assert!(queue.take_pause_request());
        assert!(!queue.take_pause_request());
        queue.set_paused(false);
        assert!(queue.permits(1));
        queue.target.close();
        assert!(!queue.submit(recovery));
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
        let events = queue.take();
        assert_eq!(events.len(), INPUT_QUEUE_CAPACITY);
        assert!(events[..INPUT_QUEUE_CAPACITY - 1]
            .iter()
            .all(|event| *event == cycle(1)));
        assert_eq!(
            events.last().unwrap().action,
            InputAction::Finish(SwitchKind::Apps)
        );
        assert_eq!(queue.queued.load(Ordering::Acquire), 0);
        queue.acknowledge(1);
        assert_eq!(queue.acknowledged(), 1);
    }

    #[test]
    fn consumer_contention_and_failed_notifications_keep_input_for_polling() {
        let queue = queue();
        let pending = queue.pending.lock();
        assert!(queue.enqueue(cycle(1), || true));
        assert!(
            queue.take().is_empty(),
            "a busy receiver must not block the UI"
        );
        drop(pending);
        assert_eq!(queue.take(), vec![cycle(1)]);
        assert!(queue.enqueue(cycle(1), || false));
        assert_eq!(queue.notification_deferrals.load(Ordering::Relaxed), 1);
        assert_eq!(queue.take(), vec![cycle(1)]);
        assert_eq!(queue.rejected.load(Ordering::Relaxed), 0);
        queue.revoked.store(1, Ordering::Release);
        assert!(!queue.enqueue(cycle(1), || true));
        queue.target.close();
        assert!(!queue.enqueue(cycle(2), || true));
        assert_eq!(queue.inactive_rejections.load(Ordering::Relaxed), 2);
        assert_eq!(queue.queue_full_rejections.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn failed_native_notifications_preserve_cycle_and_finish_order() {
        // HWND_BOTTOM is a positioning sentinel, never a message endpoint.
        let queue = InputDispatch::new(Arc::new(WindowTarget::new(HWND(1 as _))));
        let finish = InputEvent {
            session: 1,
            action: InputAction::Finish(SwitchKind::Apps),
        };
        assert!(queue.submit(cycle(1)));
        assert!(queue.submit(finish));
        assert_eq!(queue.notification_deferrals.load(Ordering::Relaxed), 2);
        assert_eq!(queue.take(), vec![cycle(1), finish]);
        assert_eq!(queue.revoked(), 0);
        assert_eq!(queue.rejected.load(Ordering::Relaxed), 0);
        queue.acknowledge(1);
        assert!(!queue.permits(1));
    }

    #[test]
    fn concurrent_delivery_preserves_order_and_bounds_each_ui_turn() {
        const EVENTS: u64 = 10_000;
        let queue = Arc::new(queue());
        let consumer_queue = queue.clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        let consumer = std::thread::spawn(move || {
            let mut received = Vec::new();
            while received.len() < EVENTS as usize {
                assert!(Instant::now() < deadline, "input delivery stalled");
                let batch = consumer_queue.take();
                assert!(batch.len() <= INPUT_QUEUE_CAPACITY);
                received.extend(batch.into_iter().map(|event| event.session));
                std::thread::yield_now();
            }
            received
        });
        for session in 1..=EVENTS {
            while !queue.enqueue(cycle(session), || true) {
                assert!(Instant::now() < deadline, "input producer stalled");
                std::thread::yield_now();
            }
        }
        assert_eq!(consumer.join().unwrap(), (1..=EVENTS).collect::<Vec<_>>());
        assert_eq!(queue.queued.load(Ordering::Acquire), 0);
        assert!(queue.take().is_empty());
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
