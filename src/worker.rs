//! Bounded latest-request mailboxes shared by the two auxiliary workers.
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};

mod request;
use request::LatestRequest;

pub(crate) struct Mailbox<Q, R> {
    generation: AtomicU64,
    closed: AtomicBool,
    request: LatestRequest<(u64, Q)>,
    results: Mutex<VecDeque<(u64, R)>>,
    waiter: OnceLock<Thread>,
    result_limit: usize,
}

impl<Q, R> Mailbox<Q, R> {
    pub(crate) fn new(result_limit: usize) -> Arc<Self> {
        assert!(result_limit > 0);
        Arc::new(Self {
            generation: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            request: LatestRequest::new(),
            results: Mutex::new(VecDeque::new()),
            waiter: OnceLock::new(),
            result_limit,
        })
    }

    pub(crate) fn request(&self, request: Q) -> u64 {
        let generation = self.advance();
        if let Some(mut results) = self.results.try_lock() {
            results.clear();
        }
        if !self.closed() {
            self.request.replace((generation, request));
        }
        self.wake();
        generation
    }

    fn advance(&self) -> u64 {
        match self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
        {
            Ok(previous) => previous + 1,
            Err(_) => {
                self.closed.store(true, Ordering::Release);
                0
            }
        }
    }

    pub(crate) fn cancel(&self) {
        self.advance();
        self.request.take();
        if let Some(mut results) = self.results.try_lock() {
            results.clear();
        }
        self.wake();
    }

    pub(crate) fn current(&self, generation: u64) -> bool {
        !self.closed() && self.generation.load(Ordering::Acquire) == generation
    }

    pub(crate) fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn receive(&self, timeout: Duration) -> Option<(u64, Q)> {
        self.waiter.get_or_init(thread::current);
        let request = self
            .request
            .take()
            .filter(|(generation, _)| self.current(*generation));
        if request.is_some() || self.closed() {
            return request;
        }
        // unpark retains a token when notification precedes park. No mutex
        // ownership is handed to a sleeping/paused worker by a UI operation.
        thread::park_timeout(timeout);
        self.request
            .take()
            .filter(|(generation, _)| self.current(*generation))
    }

    pub(crate) fn publish(&self, generation: u64, result: R) -> bool {
        loop {
            if !self.current(generation) {
                return false;
            }
            {
                // Only the producer may wait for this mutex. UI readers and
                // cancellation always use try_lock and can make progress.
                let mut results = self.results.lock();
                results.retain(|(generation, _)| self.current(*generation));
                if !self.current(generation) {
                    return false;
                }
                if results.len() < self.result_limit {
                    results.push_back((generation, result));
                    return true;
                }
            }
            self.waiter.get_or_init(thread::current);
            thread::park_timeout(Duration::from_millis(50));
        }
    }

    pub(crate) fn take(&self) -> Vec<(u64, R)> {
        let Some(mut results) = self.results.try_lock() else {
            return Vec::new();
        };
        let output = results
            .drain(..)
            .filter(|(generation, _)| self.current(*generation))
            .collect();
        drop(results);
        self.wake();
        output
    }

    fn wake(&self) {
        if let Some(waiter) = self.waiter.get() {
            waiter.unpark();
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancel();
    }
}

pub(crate) fn retire(handle: Option<JoinHandle<()>>, stage: &str) {
    if let Some(handle) = handle {
        if handle.is_finished() {
            if handle.join().is_err() {
                warn!("worker stage={stage} panicked");
            }
        } else {
            // Never join an uncancellable Shell call on the UI thread. Its
            // Arc-owned inputs outlive the UI and no replacement is spawned.
            debug!("worker stage={stage} retiring-in-flight");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newest_request_cancels_old_work_and_discards_late_results() {
        let mailbox = Mailbox::<u32, u32>::new(2);
        let old = mailbox.request(1);
        assert_eq!(mailbox.receive(Duration::ZERO), Some((old, 1)));
        let new = mailbox.request(2);
        assert!(!mailbox.publish(old, 9));
        assert_eq!(mailbox.receive(Duration::ZERO), Some((new, 2)));
        assert!(mailbox.publish(new, 3));
        assert_eq!(mailbox.take(), vec![(new, 3)]);
        mailbox.cancel();
        assert!(!mailbox.publish(new, 4));
        assert!(mailbox.take().is_empty());
    }

    #[test]
    fn a_full_result_queue_applies_backpressure_and_close_releases_it() {
        let mailbox = Mailbox::<(), u32>::new(1);
        let generation = mailbox.request(());
        assert!(mailbox.publish(generation, 1));
        let worker = mailbox.clone();
        let thread = std::thread::spawn(move || worker.publish(generation, 2));
        mailbox.close();
        assert!(!thread.join().unwrap());
        assert!(mailbox.take().is_empty());
    }

    #[test]
    fn ui_request_cancel_and_poll_never_wait_for_a_stalled_producer() {
        let mailbox = Mailbox::<u32, u32>::new(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let shared = mailbox.clone();
        let worker = thread::spawn(move || {
            let _held = shared.results.lock();
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = std::time::Instant::now();
        let generation = mailbox.request(1);
        assert!(mailbox.take().is_empty());
        mailbox.cancel();
        mailbox.close();
        let elapsed = started.elapsed();
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(500),
            "UI mailbox call waited for a producer: {elapsed:?}"
        );
        assert!(!mailbox.current(generation));
        assert!(mailbox.take().is_empty());
    }
}
