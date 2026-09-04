use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use indexmap::IndexMap;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
    UI::WindowsAndMessaging::{DestroyIcon, PostMessageW},
};

use crate::{metrics::StageTimer, utils::try_get_app_icon};

pub(crate) const WM_USER_ICON_READY: u32 = 6040;

const ICON_REQUEST_CAPACITY: usize = 32;
const ICON_RESULT_CAPACITY: usize = 64;
const ICON_RESULT_SEND_RETRIES: usize = 100;
const ICON_RESULT_SEND_DELAY: Duration = Duration::from_millis(1);
const ICON_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
struct IconRequest {
    key: String,
    module_path: String,
    hwnd: isize,
    generation: u64,
}

#[derive(Debug)]
pub(crate) struct IconLoadResult {
    pub key: String,
    pub generation: u64,
    pub hicon: Option<isize>,
}

pub(crate) struct IconLoader {
    request_tx: Option<SyncSender<IconRequest>>,
    request_rx: Option<Receiver<IconRequest>>,
    result_tx: Option<SyncSender<IconLoadResult>>,
    result_rx: Receiver<IconLoadResult>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    wake_pending: Arc<AtomicBool>,
    hwnd: isize,
    override_icons: Arc<IndexMap<String, String>>,
}

impl IconLoader {
    pub(crate) fn new(hwnd: HWND, override_icons: Arc<IndexMap<String, String>>) -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel(ICON_REQUEST_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(ICON_RESULT_CAPACITY);

        Self {
            request_tx: Some(request_tx),
            request_rx: Some(request_rx),
            result_tx: Some(result_tx),
            result_rx,
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            wake_pending: Arc::new(AtomicBool::new(false)),
            hwnd: hwnd.0 as isize,
            override_icons,
        }
    }

    pub(crate) fn request(
        &mut self,
        key: &str,
        module_path: &str,
        representative_hwnd: HWND,
        generation: u64,
    ) -> bool {
        if !self.ensure_worker() {
            return false;
        }
        let Some(request_tx) = self.request_tx.as_ref() else {
            return false;
        };
        let request = IconRequest {
            key: key.to_string(),
            module_path: module_path.to_string(),
            hwnd: representative_hwnd.0 as isize,
            generation,
        };
        match request_tx.try_send(request) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                debug!("icon loader queue full; deferring icon {key}");
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                warn!("icon loader worker disconnected; deferring icon {key}");
                false
            }
        }
    }

    fn ensure_worker(&mut self) -> bool {
        if self.worker.is_some() {
            return true;
        }

        let Some(request_rx) = self.request_rx.take() else {
            return false;
        };
        let Some(result_tx) = self.result_tx.take() else {
            return false;
        };

        let stop = Arc::clone(&self.stop);
        let wake_pending = Arc::clone(&self.wake_pending);
        let override_icons = Arc::clone(&self.override_icons);
        let hwnd = self.hwnd;
        let worker = thread::Builder::new()
            .name("window-switcher-icon-loader".to_string())
            .spawn(move || {
                run_worker(
                    request_rx,
                    result_tx,
                    override_icons,
                    hwnd,
                    stop,
                    wake_pending,
                );
            });

        match worker {
            Ok(worker) => {
                self.worker = Some(worker);
                true
            }
            Err(err) => {
                self.request_tx.take();
                warn!("failed to start icon loader worker: {err}");
                false
            }
        }
    }

    pub(crate) fn drain_results(&self) -> Vec<IconLoadResult> {
        let mut results = Vec::new();

        // Clear the wake flag only after draining. If the worker publishes a
        // result during the final probe it observes the cleared flag and posts
        // another wake message, so a result cannot get stranded in the queue.
        loop {
            results.extend(self.result_rx.try_iter());
            self.wake_pending.store(false, Ordering::Release);
            match self.result_rx.try_recv() {
                Ok(result) => results.push(result),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        results
    }

    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.request_tx.take();
        self.request_rx.take();
        self.result_tx.take();
        if let Some(worker) = self.worker.take() {
            if let Err(err) = worker.join() {
                warn!("icon loader worker panicked: {err:?}");
            }
        }
        for result in self.result_rx.try_iter() {
            if let Some(hicon) = result.hicon {
                destroy_icon(hicon);
            }
        }
        self.wake_pending.store(false, Ordering::Release);
    }
}

impl Drop for IconLoader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    request_rx: Receiver<IconRequest>,
    result_tx: SyncSender<IconLoadResult>,
    override_icons: Arc<IndexMap<String, String>>,
    hwnd: isize,
    stop: Arc<AtomicBool>,
    wake_pending: Arc<AtomicBool>,
) {
    let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    if !com_initialized {
        warn!("icon loader could not initialize COM; continuing without COM apartment");
    }

    while !stop.load(Ordering::Acquire) {
        let mut request = match request_rx.recv_timeout(ICON_WORKER_POLL_INTERVAL) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if stop.load(Ordering::Acquire) {
            break;
        }

        let mut should_wake = false;
        loop {
            let _timer = StageTimer::new("icon_load");
            let representative_hwnd = HWND(request.hwnd as _);
            let hicon =
                try_get_app_icon(&override_icons, &request.module_path, representative_hwnd)
                    .and_then(|icon| (!icon.is_invalid()).then_some(icon.0 as isize));
            let result = IconLoadResult {
                key: request.key,
                generation: request.generation,
                hicon,
            };

            should_wake |= send_result(&result_tx, result, &stop);
            if stop.load(Ordering::Acquire) {
                break;
            }
            match request_rx.try_recv() {
                Ok(next_request) => request = next_request,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }

        if should_wake {
            // One wake drains the whole completed batch and produces a single
            // static-layer repaint instead of one repaint per icon.
            post_icon_ready(hwnd, &wake_pending);
        }
    }

    if com_initialized {
        unsafe { CoUninitialize() };
    }
}

fn send_result(
    result_tx: &SyncSender<IconLoadResult>,
    mut result: IconLoadResult,
    stop: &AtomicBool,
) -> bool {
    for _ in 0..ICON_RESULT_SEND_RETRIES {
        if stop.load(Ordering::Acquire) {
            destroy_result_icon(&result);
            return false;
        }
        match result_tx.try_send(result) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                result = returned;
                thread::sleep(ICON_RESULT_SEND_DELAY);
            }
            Err(TrySendError::Disconnected(returned)) => {
                destroy_result_icon(&returned);
                return false;
            }
        }
    }

    warn!("icon loader result queue remained full; dropping icon result");
    destroy_result_icon(&result);
    false
}

fn destroy_result_icon(result: &IconLoadResult) {
    if let Some(hicon) = result.hicon {
        destroy_icon(hicon);
    }
}

fn post_icon_ready(hwnd: isize, wake_pending: &AtomicBool) {
    if hwnd == 0 || wake_pending.swap(true, Ordering::AcqRel) {
        return;
    }
    if unsafe {
        PostMessageW(
            Some(HWND(hwnd as _)),
            WM_USER_ICON_READY,
            WPARAM(0),
            LPARAM(0),
        )
    }
    .is_err()
    {
        wake_pending.store(false, Ordering::Release);
        debug!("failed to post icon loader completion message");
    }
}

fn destroy_icon(raw_hicon: isize) {
    if raw_hicon == 0 || raw_hicon == -1 {
        return;
    }
    unsafe {
        let _ = DestroyIcon(windows::Win32::UI::WindowsAndMessaging::HICON(
            raw_hicon as _,
        ));
    }
}
