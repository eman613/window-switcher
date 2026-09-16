//! A single lazy worker saves pause settings without blocking input or the UI.
use crate::{
    config::{settings::save_input_paused, Config},
    keyboard::dispatch::WM_INPUT_READY,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};
use anyhow::{ensure, Context, Result};
use std::{
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

type SaveMailbox = Mailbox<Config, Result<bool, String>>;

pub(crate) struct PauseControl {
    path: PathBuf,
    target: Arc<WindowTarget>,
    mailbox: Option<Arc<SaveMailbox>>,
    thread: Option<JoinHandle<()>>,
    pending: Option<u64>,
}

impl PauseControl {
    pub(crate) fn new(path: PathBuf, target: Arc<WindowTarget>) -> Self {
        Self {
            path,
            target,
            mailbox: None,
            thread: None,
            pending: None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn request(&mut self, config: &Config) -> Result<()> {
        ensure!(self.target.is_live(), "pause stage=save application-closed");
        if self.busy() {
            return Ok(());
        }
        if self.mailbox.is_none() {
            let mailbox = SaveMailbox::new(1);
            let shared = mailbox.clone();
            let path = self.path.clone();
            let target = self.target.clone();
            let thread = thread::Builder::new()
                .name("pause-settings".into())
                .spawn(move || {
                    while !shared.closed() && target.is_live() {
                        let Some((generation, expected)) = shared.receive(Duration::from_secs(30))
                        else {
                            continue;
                        };
                        if !shared.current(generation) || !target.is_live() {
                            continue;
                        }
                        let paused = !expected.input_paused;
                        let result = save_input_paused(&path, &expected, paused)
                            .map(|()| paused)
                            .map_err(|error| format!("{error:#}"));
                        if shared.publish(generation, result) {
                            target.try_post(WM_INPUT_READY);
                        }
                    }
                })
                .context("pause stage=thread-create")?;
            self.mailbox = Some(mailbox);
            self.thread = Some(thread);
        }
        ensure!(
            self.healthy(),
            "pause stage=save worker-unavailable; restart before retrying"
        );
        self.pending = Some(self.mailbox.as_ref().unwrap().request(config.clone()));
        Ok(())
    }

    fn healthy(&self) -> bool {
        self.mailbox
            .as_ref()
            .is_some_and(|mailbox| !mailbox.closed())
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<bool, String>> {
        let generation = self.pending?;
        for (completed, result) in self.mailbox.as_ref()?.take() {
            if generation == completed {
                self.pending = None;
                return Some(result);
            }
        }
        if !self.healthy() {
            self.pending = None;
            return Some(Err(
                "暂停设置工作者不可用；输入状态未改变，请重新启动应用后重试".into(),
            ));
        }
        None
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(mailbox) = &self.mailbox {
            mailbox.close();
        }
        self.pending = None;
    }
}

impl Drop for PauseControl {
    fn drop(&mut self) {
        self.cancel();
        worker::retire(self.thread.take(), "pause-settings");
    }
}
