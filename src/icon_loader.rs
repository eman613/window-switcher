use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
};

use indexmap::IndexMap;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
    UI::WindowsAndMessaging::{DestroyIcon, PostMessageW},
};

use crate::utils::try_get_app_icon;

pub(crate) const WM_USER_ICON_READY: u32 = 6040;

const ICON_REQUEST_CAPACITY: usize = 32;

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
    result_rx: Receiver<IconLoadResult>,
    worker: Option<JoinHandle<()>>,
}

impl IconLoader {
    pub(crate) fn new(
        hwnd: HWND,
        override_icons: Arc<IndexMap<String, String>>,
    ) -> std::io::Result<Self> {
        let (request_tx, request_rx) = mpsc::sync_channel(ICON_REQUEST_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let worker_hwnd = hwnd.0 as isize;
        let worker = thread::Builder::new()
            .name("window-switcher-icon-loader".to_string())
            .spawn(move || {
                run_worker(request_rx, result_tx, override_icons, worker_hwnd);
            })?;

        Ok(Self {
            request_tx: Some(request_tx),
            result_rx,
            worker: Some(worker),
        })
    }

    pub(crate) fn request(
        &self,
        key: &str,
        module_path: &str,
        representative_hwnd: HWND,
        generation: u64,
    ) -> bool {
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

    pub(crate) fn drain_results(&self) -> Vec<IconLoadResult> {
        self.result_rx.try_iter().collect()
    }

    pub(crate) fn stop(&mut self) {
        self.request_tx.take();
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
    }
}

impl Drop for IconLoader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    request_rx: Receiver<IconRequest>,
    result_tx: mpsc::Sender<IconLoadResult>,
    override_icons: Arc<IndexMap<String, String>>,
    hwnd: isize,
) {
    let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    if !com_initialized {
        warn!("icon loader could not initialize COM; continuing without COM apartment");
    }

    while let Ok(request) = request_rx.recv() {
        let representative_hwnd = HWND(request.hwnd as _);
        let hicon = try_get_app_icon(&override_icons, &request.module_path, representative_hwnd)
            .and_then(|icon| (!icon.is_invalid()).then_some(icon.0 as isize));
        let result = IconLoadResult {
            key: request.key,
            generation: request.generation,
            hicon,
        };
        let Some(hicon) = result.hicon else {
            if result_tx.send(result).is_err() {
                break;
            }
            post_icon_ready(hwnd);
            continue;
        };

        if result_tx.send(result).is_err() {
            destroy_icon(hicon);
            break;
        }
        post_icon_ready(hwnd);
    }

    if com_initialized {
        unsafe { CoUninitialize() };
    }
}

fn post_icon_ready(hwnd: isize) {
    if hwnd == 0 {
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
