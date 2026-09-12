use crate::utils::get_window_exe;
use anyhow::{anyhow, bail, Result};
use once_cell::sync::OnceCell;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
        },
    },
};

pub static IS_FOREGROUND_IN_BLACKLIST: AtomicBool = AtomicBool::new(false);

static FOREGROUND_WINDOW_TX: OnceCell<parking_lot::Mutex<Option<SyncSender<()>>>> = OnceCell::new();
static FOREGROUND_LATEST: AtomicIsize = AtomicIsize::new(0);
static FOREGROUND_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const FOREGROUND_QUEUE_CAPACITY: usize = 1;
const FOREGROUND_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct ForegroundWatcher {
    hook: HWINEVENTHOOK,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl ForegroundWatcher {
    pub fn init(blacklist: &HashSet<String>) -> Result<Self> {
        IS_FOREGROUND_IN_BLACKLIST.store(false, Ordering::Release);
        if blacklist.is_empty() {
            return Ok(Self {
                hook: HWINEVENTHOOK::default(),
                worker: None,
                stop: Arc::new(AtomicBool::new(false)),
            });
        }

        let (window_tx, window_rx) = mpsc::sync_channel(FOREGROUND_QUEUE_CAPACITY);
        let channel = FOREGROUND_WINDOW_TX.get_or_init(|| parking_lot::Mutex::new(None));
        {
            let mut sender = channel.lock();
            if sender.is_some() {
                return Err(anyhow!("Foreground watcher is already initialized"));
            }
            *sender = Some(window_tx.clone());
        }

        FOREGROUND_LATEST.store(0, Ordering::Release);
        FOREGROUND_SEQUENCE.store(0, Ordering::Release);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let blacklist = blacklist.iter().map(|value| value.to_lowercase()).collect();
        let worker = match thread::Builder::new()
            .name("window-switcher-foreground".to_string())
            .spawn(move || run_foreground_worker(window_rx, blacklist, worker_stop))
        {
            Ok(worker) => worker,
            Err(err) => {
                channel.lock().take();
                return Err(anyhow!("Failed to start foreground worker, {err}"));
            }
        };

        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_invalid() {
            stop.store(true, Ordering::Release);
            channel.lock().take();
            if let Err(err) = worker.join() {
                warn!("foreground worker panicked during cleanup: {err:?}");
            }
            bail!("Failed to watch foreground");
        }

        let initial = crate::utils::get_foreground_window();
        publish_foreground(initial.0 as isize);

        info!("foreground watcher start");

        Ok(Self {
            hook,
            worker: Some(worker),
            stop,
        })
    }
}

impl Drop for ForegroundWatcher {
    fn drop(&mut self) {
        debug!("foreground watcher destroyed");
        if !self.hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.hook);
            }
        }
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if let Err(err) = worker.join() {
                warn!("foreground worker panicked: {err:?}");
            }
        }
        if let Some(channel) = FOREGROUND_WINDOW_TX.get() {
            channel.lock().take();
        }
        FOREGROUND_LATEST.store(0, Ordering::Release);
        FOREGROUND_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        IS_FOREGROUND_IN_BLACKLIST.store(false, Ordering::Release);
    }
}

fn run_foreground_worker(
    window_rx: Receiver<()>,
    blacklist: HashSet<String>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        if !matches!(
            window_rx.recv_timeout(FOREGROUND_WORKER_POLL_INTERVAL),
            Ok(())
        ) {
            continue;
        }

        let mut processed_sequence = 0;
        loop {
            let sequence = FOREGROUND_SEQUENCE.load(Ordering::Acquire);
            if !should_process_sequence(processed_sequence, sequence) {
                break;
            }
            let raw_hwnd = FOREGROUND_LATEST.load(Ordering::Acquire);
            let exe = get_window_exe(HWND(raw_hwnd as _)).map(|value| value.to_lowercase());
            let is_in_blacklist = exe
                .as_ref()
                .map(|value| blacklist.contains(value))
                .unwrap_or(false);
            IS_FOREGROUND_IN_BLACKLIST.store(is_in_blacklist, Ordering::Release);
            processed_sequence = sequence;
            if FOREGROUND_SEQUENCE.load(Ordering::Acquire) != sequence {
                continue;
            }
            break;
        }
    }
}

fn should_process_sequence(processed: u64, current: u64) -> bool {
    current != 0 && current != processed
}

unsafe extern "system" fn win_event_proc(
    _h_win_event_hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _dw_event_thread: u32,
    _dwms_event_time: u32,
) {
    let raw_hwnd = hwnd.0 as isize;
    if raw_hwnd == 0 {
        return;
    }
    publish_foreground(raw_hwnd);
}

fn publish_foreground(raw_hwnd: isize) {
    if raw_hwnd == 0 {
        return;
    }
    FOREGROUND_LATEST.store(raw_hwnd, Ordering::Release);
    FOREGROUND_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    let Some(channel) = FOREGROUND_WINDOW_TX.get() else {
        return;
    };
    let Some(window_tx) = channel
        .try_lock()
        .and_then(|mut sender| sender.as_mut().cloned())
    else {
        return;
    };
    match window_tx.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) | Err(TrySendError::Disconnected(())) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::should_process_sequence;

    #[test]
    fn latest_sequence_replaces_stale_work() {
        assert!(should_process_sequence(1, 2));
        assert!(!should_process_sequence(2, 2));
        assert!(!should_process_sequence(0, 0));
    }
}
