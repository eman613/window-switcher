use super::App;
use crate::{
    icon_loader::IconRequest,
    keyboard::state::SwitchKind,
    search::SearchAction,
    utils::set_foreground_window,
    window_snapshot::{filter::WindowFilter, WindowSnapshot},
};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

impl App {
    pub(super) fn start_search(&mut self) -> Result<()> {
        let already_active = self.search.as_ref().is_some_and(|search| search.active());
        if !already_active {
            self.hide_apps();
        }
        let monitor = self
            .switching
            .monitor
            .context("search stage=monitor missing")?;
        self.search
            .as_mut()
            .context("search stage=open disabled")?
            .open(&self.config, monitor)?;
        if !already_active {
            debug!("search stage=open session={}", self.input_session);
            self.request_snapshot(SwitchKind::Search, false)?;
        }
        Ok(())
    }

    pub(super) fn apply_search_snapshot(&mut self, snapshot: WindowSnapshot) -> Result<()> {
        if let Some(search) = self.search.as_mut().filter(|search| search.active()) {
            search.snapshot(snapshot)?;
        }
        Ok(())
    }

    pub(super) fn poll_search(&mut self) -> Result<()> {
        let action = self
            .search
            .as_mut()
            .map(|search| search.poll())
            .transpose()?
            .flatten();
        match action {
            Some(SearchAction::Cancel) => self.complete_switch(),
            Some(SearchAction::Activate(entry)) => {
                self.cancel_preview();
                let identity = entry.identity;
                let filter = WindowFilter::from_config(&self.config, SwitchKind::Search)
                    .with_scope(self.switching.scope);
                if filter.allows_process(&entry.executable)
                    && identity.is_current(&self.snapshots.lifetimes)
                    && filter.allows(identity.hwnd()).is_some()
                    && set_foreground_window(identity.hwnd(), || {
                        self.input.permits(self.input_session)
                            && identity.is_current(&self.snapshots.lifetimes)
                    })
                {
                    self.complete_switch();
                } else {
                    debug!("search stage=activation stale-or-unavailable");
                    if let Some(search) = self.search.as_mut() {
                        search.invalidate()?;
                    }
                    self.request_snapshot(SwitchKind::Search, true)?;
                }
            }
            None => {}
        }
        self.request_search_icons();
        Ok(())
    }

    fn request_search_icons(&mut self) {
        let Some(search) = self.search.as_ref().filter(|search| search.active()) else {
            return;
        };
        let keys = search.visible_icon_keys();
        if keys.is_empty() {
            if !self.switching.icon_keys.is_empty() {
                self.icons.cancel();
                self.switching.icon_keys.clear();
            }
            return;
        }
        let refresh_due = self.switching.last_icons.is_none_or(|when| {
            when.elapsed()
                >= Duration::from_millis(self.config.icon_failure_ttl_ms.min(30000).into())
        });
        if self.switching.icon_keys == keys && !refresh_due {
            return;
        }
        for key in &keys {
            if let Some(image) = self
                .remembered_icons
                .get(key)
                .and_then(std::sync::Weak::upgrade)
            {
                search.apply_icon(key, image);
            }
        }
        self.switching.icon_keys = keys.clone();
        let mut requests: Vec<_> = keys
            .into_iter()
            .map(|key| IconRequest { key, image: true })
            .collect();
        if let Some(selected) = search.selected_window() {
            if let Some(index) = requests
                .iter()
                .position(|request| request.key.identity == selected)
            {
                requests.swap(0, index);
            }
        }
        self.switching.icon_generation = self.icons.request(requests);
        self.switching.last_icons = Some(Instant::now());
    }
}
