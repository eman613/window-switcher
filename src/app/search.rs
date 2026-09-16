use super::App;
use crate::{
    keyboard::state::SwitchKind,
    layout::MonitorSnapshot,
    search::SearchAction,
    utils::{get_foreground_window, set_foreground_window},
    window_snapshot::{filter::WindowFilter, WindowSnapshot},
};
use anyhow::{Context, Result};

impl App {
    pub(super) fn start_search(&mut self) -> Result<()> {
        let already_active = self.search.as_ref().is_some_and(|search| search.active());
        if !already_active {
            self.hide_apps();
            self.switching.monitor = Some(MonitorSnapshot::capture(
                &self.config,
                get_foreground_window(),
            )?);
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
                let identity = entry.identity;
                let filter = WindowFilter::from_config(&self.config, SwitchKind::Search);
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
        Ok(())
    }
}
