use std::{
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::PostMessageW,
};

use super::{document, encoding, storage, LoadedConfig};
use crate::app::WM_USER_CONFIG_CHANGED;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) enum ConfigEvent {
    Changed,
    Invalid(String),
}

pub(crate) struct ConfigWatcher {
    events: Receiver<ConfigEvent>,
    stop: Sender<()>,
}

impl ConfigWatcher {
    pub(crate) fn start(loaded: &LoadedConfig, hwnd: HWND) -> Result<Self> {
        let (events_tx, events) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel();
        let path = loaded.path.clone();
        let delay = Duration::from_millis(u64::from(loaded.config.restart_delay_ms));
        let mut changes = StableChange::new(loaded.contents.clone(), delay);
        let window = hwnd.0 as isize;
        thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                let mut read_error: Option<(String, Instant, bool)> = None;
                loop {
                    match stop_rx.recv_timeout(POLL_INTERVAL) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let bytes = match storage::read_bytes(&path) {
                        Ok(bytes) => {
                            read_error = None;
                            bytes
                        }
                        Err(err) => {
                            changes.clear_pending();
                            // Editors may briefly remove or lock the file while saving it.
                            let message = format!(
                                "config stage=watch path={} error={err:#}；保留当前配置",
                                path.display()
                            );
                            let failure = read_error
                                .get_or_insert_with(|| (message.clone(), Instant::now(), false));
                            if failure.0 != message {
                                *failure = (message, Instant::now(), false);
                            }
                            if !failure.2 && failure.1.elapsed() >= delay {
                                failure.2 = true;
                                error!("{}", failure.0);
                                if !notify(
                                    &stop_rx,
                                    &events_tx,
                                    window,
                                    ConfigEvent::Invalid(failure.0.clone()),
                                ) {
                                    break;
                                }
                            }
                            continue;
                        }
                    };
                    let Some(bytes) = changes.observe(bytes, Instant::now()) else {
                        continue;
                    };
                    let result = validate_candidate(&bytes, &path).with_context(|| {
                        format!("config stage=watch-validate path={}", path.display())
                    });
                    let event = match result {
                        Ok(true) => ConfigEvent::Changed,
                        Ok(false) => continue,
                        Err(err) => {
                            let message = format!("{err:#}；保留当前配置，修正并保存后会重新检测");
                            error!("{message}");
                            ConfigEvent::Invalid(message)
                        }
                    };
                    if !notify(&stop_rx, &events_tx, window, event) {
                        break;
                    }
                }
            })
            .context("无法启动 INI 后台监测线程")?;
        info!(
            "config stage=watch-start path={} delay_ms={}",
            loaded.path.display(),
            loaded.config.restart_delay_ms
        );
        Ok(Self { events, stop })
    }

    pub(crate) fn next_event(&self) -> Option<ConfigEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        // Wake the background worker without waiting for filesystem IO on the UI thread.
        let _ = self.stop.send(());
    }
}

fn notify(
    stop: &Receiver<()>,
    events: &Sender<ConfigEvent>,
    window: isize,
    event: ConfigEvent,
) -> bool {
    if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
        return false;
    }
    if events.send(event).is_err() {
        return false;
    }
    if let Err(err) = unsafe {
        PostMessageW(
            Some(HWND(window as _)),
            WM_USER_CONFIG_CHANGED,
            WPARAM(0),
            LPARAM(0),
        )
    } {
        error!("config stage=notify error={err}");
        return false;
    }
    true
}

fn validate_candidate(bytes: &[u8], ini_path: &Path) -> Result<bool> {
    let (text, _) = encoding::decode(bytes)?;
    let ini = document::parse_ini(&text)?;
    if ini.iter().all(|(_, properties)| properties.is_empty()) {
        return Ok(false);
    }
    let config = super::Config::load(&ini)?;
    storage::validate_log_destination(ini_path, config.log_file.as_deref())?;
    if let Some(path) = &config.log_file {
        super::prepare_log_file(path)
            .with_context(|| format!("无法写入日志 '{}'；请检查目录和权限", path.display()))?;
    }
    Ok(true)
}

struct StableChange {
    acknowledged: Vec<u8>,
    pending: Option<(Vec<u8>, Instant)>,
    delay: Duration,
}

impl StableChange {
    fn new(acknowledged: Vec<u8>, delay: Duration) -> Self {
        Self {
            acknowledged,
            pending: None,
            delay,
        }
    }

    fn clear_pending(&mut self) {
        self.pending = None;
    }

    fn observe(&mut self, bytes: Vec<u8>, now: Instant) -> Option<Vec<u8>> {
        if bytes.is_empty() || bytes == self.acknowledged {
            self.clear_pending();
            return None;
        }
        match self.pending.as_ref() {
            Some((pending, since)) if pending == &bytes => {
                if now.duration_since(*since) < self.delay {
                    return None;
                }
                self.acknowledged.clone_from(&bytes);
                self.clear_pending();
                Some(bytes)
            }
            _ => {
                self.pending = Some((bytes, now));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_saves_restart_once_after_last_change() {
        let now = Instant::now();
        let tick = Duration::from_millis(250);
        let mut changes = StableChange::new(b"old".to_vec(), tick * 4);
        assert!(changes.observe(b"first".to_vec(), now).is_none());
        assert!(changes
            .observe(b"second".to_vec(), now + tick * 2)
            .is_none());
        assert!(changes
            .observe(b"second".to_vec(), now + tick * 5)
            .is_none());
        assert_eq!(
            changes.observe(b"second".to_vec(), now + tick * 6),
            Some(b"second".to_vec())
        );
        assert!(changes
            .observe(b"second".to_vec(), now + tick * 12)
            .is_none());
    }

    #[test]
    fn same_content_and_temporary_empty_or_missing_files_do_not_restart() {
        let now = Instant::now();
        let delay = Duration::from_secs(1);
        let mut changes = StableChange::new(b"initial".to_vec(), delay);
        assert!(changes.observe(b"initial".to_vec(), now).is_none());
        assert!(changes.observe(b"new".to_vec(), now).is_none());
        assert!(changes.observe(Vec::new(), now + delay).is_none());
        assert!(changes.observe(b"new".to_vec(), now + delay * 2).is_none());
        changes.clear_pending();
        assert!(changes.observe(b"new".to_vec(), now + delay * 3).is_none());
        assert!(changes.observe(b"new".to_vec(), now + delay * 4).is_some());
    }

    #[test]
    fn invalid_or_blank_candidates_cannot_restart_into_defaults() {
        let validate = |bytes: &[u8]| validate_candidate(bytes, Path::new("window-switcher.ini"));
        assert!(!validate(b"").unwrap());
        assert!(!validate(b"; saving\n[switch-apps]\n").unwrap());
        assert!(validate(b"[switch-apps\n").is_err());
        assert!(validate(b"[switch-apps]\nenable = invalid\n").is_err());
        assert!(validate(b"[switch-apps]\nbadge_color = black\n").is_err());
        assert!(validate(b"[switch-apps]\nenable = yes\n").unwrap());
    }
}
