//! Focus-independent speech notifications; only the latest request is retained.
mod provider;
#[cfg(test)]
mod tests;

use crate::{
    utils::com::ComApartment,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};
use parking_lot::Mutex;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{
    core::{Weak, BSTR},
    Win32::{Foundation::HWND, UI::Accessibility::*},
};

struct Shared {
    target: Arc<WindowTarget>,
    active: AtomicBool,
    emitted: AtomicU64,
    name: String,
    root: Mutex<Weak<IRawElementProviderSimple>>,
}

impl Shared {
    fn available(&self) -> windows::core::Result<()> {
        if self.target.is_live() && self.active.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(super::snapshot::unavailable())
        }
    }
}

pub(crate) struct Announcer {
    shared: Arc<Shared>,
    pub(crate) root: IRawElementProviderSimple,
    requests: Arc<Mailbox<String, ()>>,
    previous: RefCell<Option<(String, u64)>>,
    thread: Option<JoinHandle<()>>,
    // Keep this thread's apartment alive through provider disconnection/release.
    _com: ComApartment,
}

impl Announcer {
    fn idle(hwnd: HWND, name: &str) -> anyhow::Result<Self> {
        let com = ComApartment::sta()?;
        let shared = Arc::new(Shared {
            target: Arc::new(WindowTarget::new(hwnd)),
            active: AtomicBool::new(false),
            emitted: AtomicU64::new(0),
            name: name.to_owned(),
            root: Mutex::new(Weak::new()),
        });
        let root = provider::root(shared.clone())?;
        let requests = Mailbox::<String, ()>::new(1);
        Ok(Self {
            shared,
            root,
            requests,
            previous: RefCell::new(None),
            thread: None,
            _com: com,
        })
    }

    pub(crate) fn new(hwnd: HWND, name: &str) -> anyhow::Result<Self> {
        let mut announcer = Self::idle(hwnd, name)?;
        let queue = announcer.requests.clone();
        let state = announcer.shared.clone();
        let thread = thread::Builder::new()
            .name("search-announcements".into())
            .spawn(move || {
                let run = || -> anyhow::Result<()> {
                    let _com = ComApartment::mta()?;
                    let provider = provider::root(state.clone())?;
                    let activity = BSTR::from("WindowSwitcher.Search");
                    while !queue.closed() && state.target.is_live() {
                        let Some((generation, value)) = queue.receive(Duration::from_secs(1))
                        else {
                            continue;
                        };
                        if !queue.current(generation)
                            || state.available().is_err()
                            || !unsafe { UiaClientsAreListening() }.as_bool()
                        {
                            continue;
                        }
                        let text = BSTR::from(value.as_str());
                        // Recheck after allocation: cancellation never waits on UIA or COM.
                        if !queue.current(generation) || state.available().is_err() {
                            continue;
                        }
                        if let Err(error) = unsafe {
                            UiaRaiseNotificationEvent(
                                &provider,
                                NotificationKind_Other,
                                NotificationProcessing_MostRecent,
                                &text,
                                &activity,
                            )
                        } {
                            warn!("uia stage=search-notification code={:#x}", error.code().0);
                        } else {
                            state.emitted.store(generation, Ordering::Release);
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    warn!("uia stage=search-worker error={error:#}");
                }
            })?;
        announcer.thread = Some(thread);
        Ok(announcer)
    }

    #[cfg(test)]
    pub(crate) fn fixture(hwnd: HWND) -> Self {
        Self::idle(hwnd, "Search fixture").unwrap()
    }

    #[cfg(test)]
    pub(crate) fn take_pending(&self) -> Option<String> {
        self.requests
            .receive(Duration::ZERO)
            .map(|(generation, value)| {
                self.shared.emitted.store(generation, Ordering::Release);
                value
            })
    }

    pub(crate) fn set_visible(&self, visible: bool) {
        self.shared.active.store(visible, Ordering::Release);
        if !visible {
            self.cancel();
        }
    }

    pub(crate) fn say(&self, value: String) {
        if value.is_empty() || self.shared.available().is_err() {
            return;
        }
        let Ok(mut previous) = self.previous.try_borrow_mut() else {
            return;
        };
        if previous.as_ref().is_some_and(|(text, generation)| {
            text == &value
                && (self.requests.current(*generation)
                    || self.shared.emitted.load(Ordering::Acquire) == *generation)
        }) {
            return;
        }
        let generation = self.requests.request(value.clone());
        *previous = Some((value, generation));
    }

    pub(crate) fn cancel(&self) {
        self.suspend();
        if let Ok(mut previous) = self.previous.try_borrow_mut() {
            *previous = None;
        }
    }

    pub(crate) fn suspend(&self) {
        self.requests.cancel();
    }

    pub(crate) fn retire(&self) {
        self.set_visible(false);
        self.shared.target.close();
        self.requests.close();
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        self.retire();
        worker::retire(self.thread.take(), "search-announcements");
        if let Err(error) = unsafe { UiaDisconnectProvider(&self.root) } {
            debug!("uia stage=search-disconnect code={:#x}", error.code().0);
        }
    }
}
