use crate::{
    config::{Config, ForegroundPolicy},
    process_metadata::ProcessMetadataCache,
    utils::{get_foreground_window, get_window_pid},
    window_snapshot::lifetimes::WindowLifetimes,
};
use anyhow::{bail, Result};
use parking_lot::Mutex;
use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            EVENT_OBJECT_CLOAKED, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
            EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW, EVENT_OBJECT_STATECHANGE,
            EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
            WINEVENT_SKIPOWNPROCESS,
        },
    },
};

thread_local! { static FOREGROUND_CONTEXT: RefCell<Option<Arc<ForegroundContext>>> = const { RefCell::new(None) }; }

#[derive(Clone, Copy)]
struct Snapshot {
    window: usize,
    version: u64,
    windows: bool,
    apps: bool,
}

pub(crate) struct ForegroundStatus {
    windows_blacklist: HashSet<String>,
    apps_blacklist: HashSet<String>,
    unknown: ForegroundPolicy,
    owner: usize,
    requested: AtomicUsize,
    version: AtomicU64,
    snapshot: Mutex<Option<Snapshot>>,
}

impl ForegroundStatus {
    fn new(config: &Config, owner: HWND) -> Self {
        Self {
            windows_blacklist: config
                .switch_windows_blacklist
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
            apps_blacklist: config
                .switch_apps_blacklist
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
            unknown: config.unknown_foreground,
            owner: owner.0 as usize,
            requested: AtomicUsize::new(get_foreground_window().0 as usize),
            version: AtomicU64::new(1),
            snapshot: Mutex::new(None),
        }
    }

    fn request(&self, hwnd: HWND) {
        if hwnd.0 as usize == self.owner {
            return;
        }
        self.requested.store(hwnd.0 as usize, Ordering::Release);
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn allows_windows(&self) -> bool {
        self.allows(false, get_foreground_window().0 as usize)
    }
    pub(crate) fn allows_apps(&self) -> bool {
        self.allows(true, get_foreground_window().0 as usize)
    }

    fn allows(&self, apps: bool, current: usize) -> bool {
        let unknown = self.unknown == ForegroundPolicy::Handle;
        if current == 0 {
            return unknown;
        }
        let blacklist = if apps {
            &self.apps_blacklist
        } else {
            &self.windows_blacklist
        };
        if blacklist.is_empty() {
            return true;
        }
        let owned = current == self.owner
            || unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindow(
                    HWND(current as _),
                    windows::Win32::UI::WindowsAndMessaging::GW_OWNER,
                )
                .is_ok_and(|owner| owner.0 as usize == self.owner)
            };
        let Some(snapshot) = self.snapshot.try_lock() else {
            return unknown;
        };
        snapshot
            .filter(|s| {
                s.version == self.version.load(Ordering::Acquire) && (s.window == current || owned)
            })
            .map_or(unknown, |s| if apps { s.apps } else { s.windows })
    }

    /// Only the snapshot worker calls this. Native event callbacks never query processes.
    pub(crate) fn resolve_pending(&self, cache: &mut ProcessMetadataCache, resolved: &mut u64) {
        let version = self.version.load(Ordering::Acquire);
        if *resolved == version {
            return;
        }
        let window = self.requested.load(Ordering::Acquire);
        let metadata = cache.lookup(get_window_pid(HWND(window as _)));
        if self.version.load(Ordering::Acquire) != version
            || self.requested.load(Ordering::Acquire) != window
        {
            return;
        }
        let snapshot = metadata.map(|metadata| Snapshot {
            window,
            version,
            windows: !self
                .windows_blacklist
                .contains(metadata.executable.as_ref()),
            apps: !self.apps_blacklist.contains(metadata.executable.as_ref()),
        });
        *self.snapshot.lock() = snapshot;
        *resolved = version;
    }
}

struct ForegroundContext {
    status: Arc<ForegroundStatus>,
    lifetimes: Arc<WindowLifetimes>,
}

pub(crate) struct ForegroundWatcher {
    hooks: Vec<HWINEVENTHOOK>,
    status: Arc<ForegroundStatus>,
}

impl ForegroundWatcher {
    pub(crate) fn init(
        config: &Config,
        owner: HWND,
        lifetimes: Arc<WindowLifetimes>,
    ) -> Result<Self> {
        let status = Arc::new(ForegroundStatus::new(config, owner));
        let context = Arc::new(ForegroundContext {
            status: status.clone(),
            lifetimes,
        });
        FOREGROUND_CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
        let mut watcher = Self {
            hooks: Vec::new(),
            status,
        };
        for (first, last) in [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_UNCLOAKED),
        ] {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(win_event_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                bail!("foreground stage=watch failed");
            }
            watcher.hooks.push(hook);
        }
        Ok(watcher)
    }
    pub(crate) fn status(&self) -> Arc<ForegroundStatus> {
        self.status.clone()
    }
}

impl Drop for ForegroundWatcher {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            if !unsafe { UnhookWinEvent(hook) }.as_bool() {
                warn!("foreground stage=unhook failed");
            }
        }
        FOREGROUND_CONTEXT.with(|slot| slot.borrow_mut().take());
        self.status.version.fetch_add(1, Ordering::AcqRel);
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    let context =
        FOREGROUND_CONTEXT.with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone()));
    let Some(context) = context else {
        return;
    };
    if event == EVENT_SYSTEM_FOREGROUND {
        context.status.request(hwnd);
    } else if object == OBJID_WINDOW.0
        && child == 0
        && !hwnd.is_invalid()
        && matches!(
            event,
            EVENT_OBJECT_CREATE
                | EVENT_OBJECT_DESTROY
                | EVENT_OBJECT_SHOW
                | EVENT_OBJECT_HIDE
                | EVENT_OBJECT_NAMECHANGE
                | EVENT_OBJECT_STATECHANGE
                | EVENT_OBJECT_CLOAKED
                | EVENT_OBJECT_UNCLOAKED
        )
    {
        context.lifetimes.event(
            hwnd.0 as usize,
            matches!(event, EVENT_OBJECT_CREATE | EVENT_OBJECT_DESTROY),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_blacklists_and_unknown_foregrounds_do_not_reuse_old_conclusions() {
        let config = Config {
            switch_apps_blacklist: HashSet::from(["games.exe".into()]),
            switch_windows_blacklist: HashSet::from(["editor.exe".into()]),
            ..Default::default()
        };
        let status = ForegroundStatus::new(&config, HWND(99usize as _));
        let version = status.version.load(Ordering::Acquire);
        *status.snapshot.lock() = Some(Snapshot {
            window: 10,
            version,
            windows: true,
            apps: false,
        });
        assert!(status.allows(false, 10));
        assert!(!status.allows(true, 10));
        assert!(status.allows(false, 99));
        assert!(!status.allows(false, 11));
        assert!(!status.allows(false, 0));
        status.request(HWND(11usize as _));
        assert!(!status.allows(false, 10));
        assert!(!status.allows(false, 99));
        let handle = ForegroundStatus::new(
            &Config {
                unknown_foreground: ForegroundPolicy::Handle,
                ..config
            },
            HWND::default(),
        );
        assert!(handle.allows(true, 11));
    }
}
