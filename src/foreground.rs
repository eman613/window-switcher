use crate::utils::{get_foreground_window, get_window_exe};
use anyhow::{bail, Result};
use parking_lot::Mutex;
use std::{cell::RefCell, collections::HashSet, sync::Arc};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
        },
    },
};

thread_local! {
    static FOREGROUND_CONTEXT: RefCell<Option<Arc<ForegroundContext>>> = const { RefCell::new(None) };
}

pub(crate) struct ForegroundStatus {
    unfiltered: bool,
    owner: isize,
    snapshot: Mutex<Option<(isize, bool)>>,
}

impl ForegroundStatus {
    pub(crate) fn allows_windows(&self) -> bool {
        if self.unfiltered {
            return true;
        }
        let current = get_foreground_window().0 as isize;
        let Some(snapshot) = self.snapshot.try_lock() else {
            return false;
        };
        snapshot.is_some_and(|(window, allowed)| {
            allowed && (window == current || current == self.owner)
        })
    }
}

struct ForegroundContext {
    blacklist: HashSet<String>,
    status: Arc<ForegroundStatus>,
}

impl ForegroundContext {
    fn update(&self, hwnd: HWND) {
        *self.status.snapshot.lock() = None;
        let Some(exe) = get_window_exe(hwnd) else {
            return;
        };
        if get_foreground_window() == hwnd {
            *self.status.snapshot.lock() = Some((
                hwnd.0 as isize,
                !self.blacklist.contains(&exe.to_lowercase()),
            ));
        }
    }
}

pub struct ForegroundWatcher {
    hook: HWINEVENTHOOK,
    status: Arc<ForegroundStatus>,
}

impl ForegroundWatcher {
    pub fn init(blacklist: &HashSet<String>, owner: HWND) -> Result<Self> {
        let status = Arc::new(ForegroundStatus {
            unfiltered: blacklist.is_empty(),
            owner: owner.0 as isize,
            snapshot: Mutex::new(None),
        });
        if blacklist.is_empty() {
            return Ok(Self {
                hook: HWINEVENTHOOK::default(),
                status,
            });
        }

        let context = Arc::new(ForegroundContext {
            blacklist: blacklist.iter().map(|v| v.to_lowercase()).collect(),
            status: status.clone(),
        });
        context.update(get_foreground_window());
        FOREGROUND_CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));

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
            FOREGROUND_CONTEXT.with(|slot| slot.borrow_mut().take());
            bail!("Failed to watch foreground");
        }

        info!("foreground watcher start");

        Ok(Self { hook, status })
    }

    pub(crate) fn status(&self) -> Arc<ForegroundStatus> {
        self.status.clone()
    }
}

impl Drop for ForegroundWatcher {
    fn drop(&mut self) {
        debug!("foreground watcher destroyed");
        if !self.hook.is_invalid() {
            unsafe {
                if !UnhookWinEvent(self.hook).as_bool() {
                    warn!("foreground stage=unhook failed");
                }
            }
        }
        FOREGROUND_CONTEXT.with(|slot| slot.borrow_mut().take());
        *self.status.snapshot.lock() = None;
    }
}

unsafe extern "system" fn win_event_proc(
    _h_win_event_hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _dw_event_thread: u32,
    _dwms_event_time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND || hwnd.is_invalid() {
        return;
    }
    let context =
        FOREGROUND_CONTEXT.with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone()));
    if let Some(context) = context {
        context.update(hwnd);
    }
}
