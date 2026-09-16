use crate::{icon_cache::IconKey, layout::PixelRect, window_target::WindowTarget};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use windows::{
    core::{Error, Result, Weak, HRESULT},
    Win32::UI::Accessibility::{
        IRawElementProviderSimple, UIA_E_ELEMENTNOTAVAILABLE, UIA_E_INVALIDOPERATION,
    },
};

pub(crate) const WM_ACCESSIBILITY: u32 = 6014;
const MAX_ACTIONS: usize = 32;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) id: u32,
    pub(super) key: IconKey,
    pub(super) name: Arc<str>,
    pub(super) status: Arc<str>,
    pub(super) bounds: Option<PixelRect>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) session: u64,
    pub(super) focused: bool,
    pub(super) bounds: PixelRect,
    pub(super) entries: Vec<Entry>,
    pub(super) selected: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) enum ActionKind {
    Select,
    Invoke,
}

pub(crate) struct Action {
    pub(crate) session: u64,
    pub(crate) key: IconKey,
    pub(crate) kind: ActionKind,
}

pub(super) struct Bridge {
    pub(super) root: Mutex<Weak<IRawElementProviderSimple>>,
    pub(super) items: Mutex<HashMap<(u64, u32), Weak<IRawElementProviderSimple>>>,
    pub(super) name: Arc<str>,
    pub(super) help: Arc<str>,
    pub(super) snapshot: RwLock<Arc<Snapshot>>,
    pub(super) target: Arc<WindowTarget>,
    pub(super) active: AtomicU64,
    actions: Mutex<VecDeque<Action>>,
}

pub(super) fn unavailable() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32).into()
}

impl Bridge {
    pub(super) fn new(target: Arc<WindowTarget>, text: crate::localization::Text) -> Arc<Self> {
        Arc::new(Self {
            root: Mutex::new(Weak::new()),
            items: Mutex::new(HashMap::new()),
            name: text.switcher_name().into(),
            help: text.switcher_help().into(),
            snapshot: RwLock::new(Arc::new(Snapshot::default())),
            target,
            active: AtomicU64::new(0),
            actions: Mutex::new(VecDeque::with_capacity(MAX_ACTIONS)),
        })
    }
    pub(super) fn read(&self) -> Result<Arc<Snapshot>> {
        if !self.target.is_live() {
            return Err(unavailable());
        }
        // Clone only; no provider callbacks or native calls while holding the lock.
        Ok(self.snapshot.read().clone())
    }
    pub(super) fn visible(&self, session: u64) -> bool {
        session != 0 && self.target.is_live() && self.active.load(Ordering::Acquire) == session
    }
    pub(super) fn send(&self, session: u64, id: u32, kind: ActionKind) -> Result<()> {
        let snapshot = self.read()?;
        if !self.visible(session) || snapshot.session != session {
            return Err(unavailable());
        }
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(unavailable)?;
        let mut actions = self
            .actions
            .try_lock()
            .ok_or_else(|| Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32)))?;
        if actions.len() >= MAX_ACTIONS {
            return Err(HRESULT(UIA_E_INVALIDOPERATION as i32).into());
        }
        if !self.visible(session) {
            return Err(unavailable());
        }
        actions.push_back(Action {
            session,
            key: entry.key.clone(),
            kind,
        });
        drop(actions);
        // The input timer retries a full OS queue without blocking this COM caller.
        self.target.try_post(WM_ACCESSIBILITY);
        Ok(())
    }
    pub(super) fn take(&self) -> Vec<Action> {
        self.actions
            .try_lock()
            .map(|mut actions| actions.drain(..).collect())
            .unwrap_or_default()
    }
    pub(super) fn hide(&self) {
        self.active.store(0, Ordering::Release);
        if let Some(mut items) = self.items.try_lock() {
            items.clear();
        }
        if let Some(mut actions) = self.actions.try_lock() {
            actions.clear();
        }
    }
}
