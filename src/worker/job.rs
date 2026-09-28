//! A lazy, single in-flight task for settings operations. UI calls never join it.
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{ensure, Context, Result};

use super::{retire, Mailbox};
use crate::window_target::WindowTarget;

type Operation<Q, R> = Box<dyn Fn(Q) -> R + Send>;

pub(crate) struct JobWorker<Q, R> {
    name: &'static str,
    target: Arc<WindowTarget>,
    message: u32,
    operation: Option<Operation<Q, R>>,
    mailbox: Arc<Mailbox<Q, R>>,
    thread: Option<JoinHandle<()>>,
    pending: Option<u64>,
}

impl<Q: Send + Sync + 'static, R: Send + 'static> JobWorker<Q, R> {
    pub(crate) fn new(
        name: &'static str,
        target: Arc<WindowTarget>,
        message: u32,
        operation: impl Fn(Q) -> R + Send + 'static,
    ) -> Self {
        Self {
            name,
            target,
            message,
            operation: Some(Box::new(operation)),
            mailbox: Mailbox::new(1),
            thread: None,
            pending: None,
        }
    }

    pub(crate) fn request(&mut self, request: Q) -> Result<()> {
        ensure!(
            self.target.is_live() && !self.mailbox.closed(),
            "settings stage=request application-closed"
        );
        ensure!(self.pending.is_none(), "设置操作正在进行；请稍后重试");
        if let Some(operation) = self.operation.take() {
            let shared = self.mailbox.clone();
            let target = self.target.clone();
            let message = self.message;
            let thread = thread::Builder::new()
                .name(self.name.into())
                .spawn(move || {
                    while !shared.closed() && target.is_live() {
                        let Some((generation, request)) = shared.receive(Duration::from_secs(30))
                        else {
                            continue;
                        };
                        if !shared.current(generation) || !target.is_live() {
                            continue;
                        }
                        let result = operation(request);
                        if shared.publish(generation, result) {
                            target.try_post(message);
                        }
                    }
                })
                .context("无法启动设置工作者")?;
            self.thread = Some(thread);
        }
        ensure!(self.healthy(), "设置工作者不可用；请重新启动应用后重试");
        self.pending = Some(self.mailbox.request(request));
        Ok(())
    }

    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn healthy(&self) -> bool {
        !self.mailbox.closed()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<R>> {
        let generation = self.pending?;
        for (completed, result) in self.mailbox.take() {
            if generation == completed {
                self.pending = None;
                return Some(Ok(result));
            }
        }
        if !self.healthy() {
            self.pending = None;
            return Some(Err(anyhow::anyhow!(
                "设置工作者不可用；请重新启动应用后重试"
            )));
        }
        None
    }
}

impl<Q, R> JobWorker<Q, R> {
    pub(crate) fn cancel(&mut self) {
        self.mailbox.close();
        self.pending = None;
    }
}

impl<Q, R> Drop for JobWorker<Q, R> {
    fn drop(&mut self) {
        self.cancel();
        retire(self.thread.take(), self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Instant};
    use windows::Win32::Foundation::HWND;

    #[test]
    fn lazy_worker_rejects_overlapping_saves_and_cancellation_does_not_wait_for_io() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut worker = JobWorker::new(
            "job-fixture",
            Arc::new(WindowTarget::new(HWND::default())),
            1,
            move |_: ()| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        );
        assert!(worker.thread.is_none());
        worker.request(()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(worker.request(()).is_err());
        let started = Instant::now();
        worker.cancel();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(!worker.busy());
        release_tx.send(()).unwrap();
        assert!(worker.request(()).is_err());
    }
}
