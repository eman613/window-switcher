use crate::utils::get_window_exe;
use anyhow::{anyhow, bail, Result};
use once_cell::sync::OnceCell;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
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

static FOREGROUND_WINDOW_TX: OnceCell<SyncSender<isize>> = OnceCell::new();
static FOREGROUND_PENDING: AtomicIsize = AtomicIsize::new(0);

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
        if blacklist.is_empty() {
            IS_FOREGROUND_IN_BLACKLIST.store(false, Ordering::Release);
            return Ok(Self {
                hook: HWINEVENTHOOK::default(),
                worker: None,
                stop: Arc::new(AtomicBool::new(false)),
            });
        }

        let (window_tx, window_rx) = mpsc::sync_channel(FOREGROUND_QUEUE_CAPACITY);
        FOREGROUND_WINDOW_TX
            .set(window_tx)
            .map_err(|_| anyhow!("Foreground watcher is already initialized"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let blacklist = blacklist.iter().map(|value| value.to_lowercase()).collect();
        let worker = thread::Builder::new()
            .name("window-switcher-foreground".to_string())
            .spawn(move || run_foreground_worker(window_rx, blacklist, worker_stop))
            .map_err(|err| anyhow!("Failed to start foreground worker, {err}"))?;

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
            if let Err(err) = worker.join() {
                warn!("foreground worker panicked during cleanup: {err:?}");
            }
            bail!("Failed to watch foreground");
        }

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
        FOREGROUND_PENDING.store(0, Ordering::Release);
        IS_FOREGROUND_IN_BLACKLIST.store(false, Ordering::Release);
    }
}

fn run_foreground_worker(
    window_rx: Receiver<isize>,
    blacklist: HashSet<String>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        let mut raw_hwnd = match window_rx.recv_timeout(FOREGROUND_WORKER_POLL_INTERVAL) {
            Ok(raw_hwnd) => raw_hwnd,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        loop {
            let pending = FOREGROUND_PENDING.swap(0, Ordering::AcqRel);
            if pending == 0 || pending == raw_hwnd {
                break;
            }
            raw_hwnd = pending;
        }

        let exe = get_window_exe(HWND(raw_hwnd as _)).map(|value| value.to_lowercase());
        let is_in_blacklist = exe
            .as_ref()
            .map(|value| blacklist.contains(value))
            .unwrap_or(false);
        IS_FOREGROUND_IN_BLACKLIST.store(is_in_blacklist, Ordering::Release);
    }
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
    let Some(window_tx) = FOREGROUND_WINDOW_TX.get() else {
        return;
    };
    match window_tx.try_send(raw_hwnd) {
        Ok(()) => {}
        Err(TrySendError::Full(raw_hwnd)) => {
            FOREGROUND_PENDING.store(raw_hwnd, Ordering::Release);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}
