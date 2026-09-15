use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};

use crate::{
    config::{settings::save_startup_enabled, Config},
    utils::scheduled_task::current_user_sid,
    window_target::WindowTarget,
};

mod backend;
mod transaction;

pub(crate) const WM_STARTUP: u32 = 6005;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StartupState {
    #[default]
    Pending,
    Ready(bool),
    Saved(bool),
    Failed,
}

pub(crate) enum StartupUpdate {
    Inspected(Result<bool, String>),
    Saved(Result<(), String>),
}

#[derive(Default)]
pub(crate) struct Startup {
    pub(crate) state: StartupState,
    events: Option<Receiver<StartupUpdate>>,
    canceled: Arc<AtomicBool>,
    configuration: Option<Config>,
    path: PathBuf,
    target: Option<Arc<WindowTarget>>,
}

impl Startup {
    pub(crate) fn start(
        configuration: &Config,
        path: PathBuf,
        target: Arc<WindowTarget>,
        admin: bool,
    ) -> Result<Self> {
        let (tx, events) = mpsc::sync_channel(1);
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let worker_config = configuration.clone();
        let worker_path = path.clone();
        let worker_target = target.clone();
        thread::Builder::new()
            .name("startup-query".into())
            .spawn(move || {
                let result = (|| -> Result<bool> {
                    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
                    let user = current_user_sid()?;
                    let mut backend = backend::WindowsBackend {
                        timeout: Duration::from_millis(
                            worker_config.startup_command_timeout_ms.into(),
                        ),
                        canceled: worker_cancel.clone(),
                        path: worker_path,
                        configuration: worker_config.clone(),
                    };
                    transaction::reconcile(&mut backend, &worker_config, &executable, &user, admin)
                })();
                if !worker_cancel.load(Ordering::Acquire) {
                    let _ = tx.try_send(StartupUpdate::Inspected(
                        result.map_err(|error| format!("{error:#}")),
                    ));
                    worker_target.try_post(WM_STARTUP);
                }
            })
            .context("无法启动自启动后台检测")?;
        Ok(Self {
            state: StartupState::Pending,
            events: Some(events),
            canceled,
            configuration: Some(configuration.clone()),
            path,
            target: Some(target),
        })
    }

    pub(crate) fn poll(&mut self) -> Option<StartupUpdate> {
        let result = match self.events.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                let message = "自启动后台工作者已退出，状态未确认；请重新启动应用后重试".into();
                if self.state == StartupState::Pending {
                    StartupUpdate::Inspected(Err(message))
                } else {
                    StartupUpdate::Saved(Err(message))
                }
            }
        };
        match &result {
            StartupUpdate::Inspected(Ok(enabled)) => self.state = StartupState::Ready(*enabled),
            StartupUpdate::Inspected(Err(_)) => self.state = StartupState::Failed,
            StartupUpdate::Saved(Ok(())) => {
                // Actual OS state is unchanged until the replacement applies the
                // saved INI. Keep the old checkmark and disable repeated toggles.
                if let StartupState::Ready(enabled) = self.state {
                    self.state = StartupState::Saved(enabled);
                }
            }
            StartupUpdate::Saved(Err(_)) => {}
        }
        self.events = None;
        Some(result)
    }

    pub(crate) fn toggle(&mut self) -> Result<()> {
        if self.canceled.load(Ordering::Acquire) {
            bail!("应用正在退出；自启动设置未修改");
        }
        let StartupState::Ready(enabled) = self.state else {
            bail!("自启动状态尚未确认，或设置正在等待重启生效");
        };
        if self.events.is_some() {
            bail!("自启动设置正在保存，请稍后再试");
        }
        let configuration = self.configuration.clone().context("自启动尚未初始化")?;
        let path = self.path.clone();
        let target = self.target.clone().context("自启动窗口已关闭")?;
        let canceled = self.canceled.clone();
        let (tx, events) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("startup-save".into())
            .spawn(move || {
                if canceled.load(Ordering::Acquire) {
                    return;
                }
                let result = save_startup_enabled(&path, &configuration, !enabled)
                    .map_err(|error| format!("{error:#}"));
                if !canceled.load(Ordering::Acquire) {
                    let _ = tx.try_send(StartupUpdate::Saved(result));
                    target.try_post(WM_STARTUP);
                }
            })
            .context("无法启动自启动设置保存线程")?;
        self.events = Some(events);
        Ok(())
    }

    pub(crate) fn busy(&self) -> bool {
        self.events.is_some()
    }

    pub(crate) fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests;
