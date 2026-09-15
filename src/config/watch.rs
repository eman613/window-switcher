use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use parking_lot::Mutex;

use super::{
    notifications::DirectoryNotification,
    reload::{load_snapshot, ConfigCandidate},
    storage,
    watch_state::ChangeTracker,
    LoadedConfig, WatchMode,
};
use crate::{app::WM_USER_CONFIG_CHANGED, window_target::WindowTarget};

pub(crate) enum ConfigEvent {
    Candidate(ConfigCandidate),
    Invalid(String),
}

enum Control {
    Stop,
    Complete { generation: u64, applied: bool },
}

#[derive(Default)]
struct PendingEvents {
    candidate: Option<ConfigCandidate>,
    invalid: Option<String>,
    stopped: bool,
}

pub(crate) struct ConfigWatcher {
    pending: Arc<Mutex<PendingEvents>>,
    control: SyncSender<Control>,
    latest: Arc<AtomicU64>,
    #[cfg(test)]
    stopped: Receiver<()>,
}

impl ConfigWatcher {
    pub(crate) fn start(loaded: &LoadedConfig, target: Arc<WindowTarget>) -> Result<Self> {
        let pending = Arc::new(Mutex::new(PendingEvents::default()));
        let latest = Arc::new(AtomicU64::new(0));
        let (control, commands) = mpsc::sync_channel(4);
        #[cfg(test)]
        let (stopped_tx, stopped) = mpsc::sync_channel(1);
        let path = loaded.path.clone();
        let delay = Duration::from_millis(loaded.config.restart_delay_ms.into());
        let interval = Duration::from_millis(loaded.config.config_poll_interval_ms.into());
        let mode = loaded.config.config_watch_mode;
        let changes = ChangeTracker::new(
            loaded.contents.clone(),
            delay,
            Duration::from_millis(loaded.config.config_retry_delay_ms.into()),
            loaded.config.config_retry_limit,
        );
        let worker = WatchWorker {
            pending: pending.clone(),
            latest: latest.clone(),
            target,
            commands,
            changes,
        };
        thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                worker.run(path, delay, interval, mode);
                #[cfg(test)]
                let _ = stopped_tx.try_send(());
            })
            .context("无法启动 INI 后台监测线程")?;
        info!(
            "config stage=watch-start delay_ms={} poll_ms={} mode={mode:?}",
            delay.as_millis(),
            interval.as_millis()
        );
        Ok(Self {
            pending,
            control,
            latest,
            #[cfg(test)]
            stopped,
        })
    }

    pub(crate) fn next_event(&self) -> Option<ConfigEvent> {
        let mut pending = self.pending.lock();
        pending
            .candidate
            .take()
            .map(ConfigEvent::Candidate)
            .or_else(|| pending.invalid.take().map(ConfigEvent::Invalid))
    }
    pub(crate) fn latest(&self) -> Arc<AtomicU64> {
        self.latest.clone()
    }
    pub(crate) fn complete(&self, generation: u64, applied: bool) {
        if self
            .control
            .try_send(Control::Complete {
                generation,
                applied,
            })
            .is_err()
        {
            error!("config stage=watch-ack unavailable");
        }
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        // Synchronize with notify so no messages are posted after stop returns.
        *self.pending.lock() = PendingEvents {
            stopped: true,
            ..Default::default()
        };
        let _ = self.control.try_send(Control::Stop);
    }
}

#[cfg(test)]
mod tests;

struct WatchWorker {
    pending: Arc<Mutex<PendingEvents>>,
    latest: Arc<AtomicU64>,
    target: Arc<WindowTarget>,
    commands: Receiver<Control>,
    changes: ChangeTracker,
}

impl WatchWorker {
    fn run(
        mut self,
        path: std::path::PathBuf,
        delay: Duration,
        interval: Duration,
        mode: WatchMode,
    ) {
        let mut notification = None;
        let mut next_notification = Instant::now();
        let mut next_poll = Instant::now();
        let mut read_error: Option<Instant> = None;
        let mut reported = false;
        loop {
            match self.commands.recv_timeout(Duration::from_millis(25)) {
                Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Control::Complete {
                    generation,
                    applied,
                }) => {
                    self.changes
                        .complete(generation, applied, true, Instant::now());
                    info!("config stage=apply-result generation={generation} applied={applied}");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if !self.target.is_live() {
                break;
            }
            let now = Instant::now();
            if mode != WatchMode::Poll && notification.is_none() && now >= next_notification {
                match DirectoryNotification::new(&path) {
                    Ok(value) => notification = Some(value),
                    Err(_) => {
                        if mode == WatchMode::Notify {
                            self.notify(ConfigEvent::Invalid("INI 目录通知不可用；请修复目录权限，或将 [config] watch_mode 改为 auto/poll 后重新启动。".into()));
                            break;
                        }
                        warn!("config stage=notify fallback=poll");
                        next_notification = now + Duration::from_secs(30);
                    }
                }
            }
            let changed = match notification.as_ref().map(DirectoryNotification::changed) {
                Some(Ok(changed)) => changed,
                Some(Err(_)) => {
                    notification = None;
                    next_notification = now + Duration::from_secs(30);
                    if mode == WatchMode::Notify {
                        self.notify(ConfigEvent::Invalid(
                            "INI 目录通知已失效；请重新启动应用或使用 auto/poll。".into(),
                        ));
                        break;
                    }
                    warn!("config stage=notify lost fallback=poll");
                    true
                }
                None => false,
            };
            if changed || now >= next_poll {
                next_poll = now + interval;
                match storage::read_bytes(&path) {
                    Ok(bytes) => {
                        read_error = None;
                        reported = false;
                        self.changes.observe(bytes, now);
                    }
                    Err(_) => {
                        self.changes.unavailable();
                        let since = read_error.get_or_insert(now);
                        if !reported && now.duration_since(*since) >= delay {
                            reported = true;
                            self.notify(ConfigEvent::Invalid("INI 暂时无法读取；保留当前配置。请检查文件是否存在及读写权限，恢复后会继续检测。".into()));
                        }
                    }
                }
                self.latest.store(self.changes.observed, Ordering::Release);
            }
            let Some(candidate) = self.changes.next(now) else {
                continue;
            };
            match load_snapshot(&candidate.contents, &path) {
                Ok(_) => {
                    self.changes.validate(candidate.generation);
                    info!(
                        "config stage=validated observed={} validated={} applied={}",
                        self.changes.observed,
                        self.changes.validated,
                        self.changes.applied_generation
                    );
                    self.notify(ConfigEvent::Candidate(candidate));
                }
                Err(error) => {
                    self.changes
                        .complete(candidate.generation, false, false, now);
                    error!(
                        "config stage=watch invalid-candidate generation={}",
                        candidate.generation
                    );
                    self.notify(ConfigEvent::Invalid(format!(
                        "{error:#}；保留当前配置，修正并保存后重试"
                    )));
                }
            }
        }
    }

    fn notify(&self, event: ConfigEvent) {
        let mut pending = self.pending.lock();
        if pending.stopped {
            return;
        }
        match event {
            ConfigEvent::Candidate(candidate) => pending.candidate = Some(candidate),
            ConfigEvent::Invalid(message) => pending.invalid = Some(message),
        }
        // A read error must not replace an undelivered in-flight candidate: only
        // the UI can acknowledge that candidate and unblock subsequent versions.
        self.target.try_post(WM_USER_CONFIG_CHANGED);
    }
}
