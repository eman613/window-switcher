use std::{
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::PostMessageW,
};

use crate::config_file::{config_file_stamp, ConfigFileStamp, ConfigSource};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

struct WatcherSignal {
    stopping: Mutex<bool>,
    wake: Condvar,
}

pub(crate) struct ConfigWatcher {
    signal: Arc<WatcherSignal>,
    worker: Option<JoinHandle<()>>,
}

impl ConfigWatcher {
    pub(crate) fn start(source: &ConfigSource, hwnd: HWND, message: u32) -> Result<Self> {
        let signal = Arc::new(WatcherSignal {
            stopping: Mutex::new(false),
            wake: Condvar::new(),
        });
        let worker_signal = Arc::clone(&signal);
        let path = source.path.clone();
        let initial_stamp = source.stamp.clone();
        let raw_hwnd = hwnd.0 as isize;
        let worker = thread::Builder::new()
            .name("config-watcher".to_string())
            .spawn(move || watch_config_file(worker_signal, path, initial_stamp, raw_hwnd, message))
            .context("Failed to start config watcher")?;

        Ok(Self {
            signal,
            worker: Some(worker),
        })
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        {
            let mut stopping = self.signal.stopping.lock();
            *stopping = true;
            self.signal.wake.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            if let Err(err) = worker.join() {
                warn!("config watcher panicked during cleanup: {err:?}");
            }
        }
    }
}

fn watch_config_file(
    signal: Arc<WatcherSignal>,
    path: PathBuf,
    mut observed: ConfigFileStamp,
    raw_hwnd: isize,
    message: u32,
) {
    let mut candidate: Option<(ConfigFileStamp, Instant)> = None;
    let mut inspection_error_reported = false;
    loop {
        {
            let mut stopping = signal.stopping.lock();
            if *stopping {
                break;
            }
            signal.wake.wait_for(&mut stopping, POLL_INTERVAL);
            if *stopping {
                break;
            }
        }

        let current = match config_file_stamp(&path) {
            Ok(stamp) => {
                inspection_error_reported = false;
                stamp
            }
            Err(err) => {
                if !inspection_error_reported {
                    warn!("failed to inspect watched config file: {err:#}");
                    inspection_error_reported = true;
                }
                continue;
            }
        };
        if current == observed {
            candidate = None;
            continue;
        }

        match candidate.as_ref() {
            Some((pending, since)) if pending == &current && since.elapsed() >= RELOAD_DEBOUNCE => {
                let hwnd = HWND(raw_hwnd as _);
                if let Err(err) = unsafe {
                    PostMessageW(Some(hwnd), message, WPARAM::default(), LPARAM::default())
                } {
                    warn!("failed to notify config change: {err}");
                    break;
                }
                observed = current;
                candidate = None;
            }
            Some((pending, _)) if pending == &current => {}
            _ => candidate = Some((current, Instant::now())),
        }
    }
}
