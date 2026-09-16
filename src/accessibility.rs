//! Native UIA providers read immutable values and queue bounded identity-checked actions.
use crate::{
    app::SwitchAppsState, layout::LayoutSnapshot, localization::Text, window_target::WindowTarget,
};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
};
use windows::Win32::UI::Accessibility::{IRawElementProviderSimple, UiaDisconnectProvider};

mod abi;
mod arrays;
mod events;
mod node;
mod provider;
mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use snapshot::{Action, ActionKind, WM_ACCESSIBILITY};
use snapshot::{Bridge, Entry, Snapshot};

pub(crate) struct Accessibility {
    shared: Arc<Bridge>,
    pub(crate) root: IRawElementProviderSimple,
    events: Option<events::Events>,
    text: Text,
    next_id: u32,
}

impl Accessibility {
    pub(crate) fn new(target: Arc<WindowTarget>, text: Text) -> windows::core::Result<Self> {
        let shared = Bridge::new(target, text);
        let root = provider::root(shared.clone())?;
        let events = match events::Events::start(shared.clone()) {
            Ok(events) => Some(events),
            Err(error) => {
                warn!("uia stage=events unavailable={error:#}");
                None
            }
        };
        Ok(Self {
            shared,
            root,
            events,
            text,
            next_id: 1,
        })
    }

    pub(crate) fn publish(
        &mut self,
        state: &SwitchAppsState,
        layout: &LayoutSnapshot,
        session: u64,
    ) -> bool {
        if session == 0 || !self.shared.target.is_live() {
            self.hide();
            return true;
        }
        let Some(previous) = self
            .shared
            .snapshot
            .try_read()
            .map(|snapshot| snapshot.clone())
        else {
            return false;
        };
        let old: HashMap<_, _> = previous
            .entries
            .iter()
            .filter(|_| previous.session == session)
            .map(|entry| (&entry.key, entry))
            .collect();
        let positions: HashMap<_, _> = layout
            .items
            .iter()
            .map(|item| {
                let mut rect = item.outer;
                rect.left = rect.left.saturating_add(layout.bounds.left);
                rect.right = rect.right.saturating_add(layout.bounds.left);
                rect.top = rect.top.saturating_add(layout.bounds.top);
                rect.bottom = rect.bottom.saturating_add(layout.bounds.top);
                (item.index, rect)
            })
            .collect();
        let mut entries = Vec::with_capacity(state.apps.len());
        for (index, app) in state.apps.iter().enumerate() {
            let existing = old.get(&app.key);
            let id = if let Some(entry) = existing {
                entry.id
            } else {
                let Some(next) = self.next_id.checked_add(1) else {
                    error!("uia stage=identity exhausted");
                    self.hide();
                    return true;
                };
                let id = self.next_id;
                self.next_id = next;
                id
            };
            let status = self.text.window_count(app.window_count);
            entries.push(Entry {
                id,
                key: app.key.clone(),
                name: app.display_name.clone(),
                status: status.into(),
                bounds: positions.get(&index).copied(),
            });
        }
        let selected = entries.get(state.index).map(|entry| entry.id);
        let snapshot = Arc::new(Snapshot {
            session,
            focused: unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0
                as usize
                == self.shared.target.window_id(),
            entries,
            selected,
            bounds: layout.bounds,
        });
        let Some(mut current) = self.shared.snapshot.try_write() else {
            return false;
        };
        let changed = **current != *snapshot;
        *current = snapshot.clone();
        self.shared.active.store(session, Ordering::Release);
        drop(current);
        if changed {
            if let Some(events) = &self.events {
                events.notify(snapshot);
            }
        }
        true
    }
    pub(crate) fn take(&self) -> Vec<Action> {
        self.shared.take()
    }
    pub(crate) fn hide(&self) {
        self.shared.hide();
        let empty = Arc::new(Snapshot::default());
        if let Some(mut current) = self.shared.snapshot.try_write() {
            *current = empty.clone();
        }
        if let Some(events) = &self.events {
            events.notify(empty);
        }
    }
}

impl Drop for Accessibility {
    fn drop(&mut self) {
        self.hide();
        if let Err(error) = unsafe { UiaDisconnectProvider(&self.root) } {
            debug!("uia stage=disconnect code={:#x}", error.code().0);
        }
    }
}
