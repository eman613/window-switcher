use super::App;
use crate::{
    keyboard::state::{InputSurface, SwitchKind},
    utils::{get_foreground_window, set_foreground_window},
    window_details::DetailsAction,
    window_snapshot::filter::WindowFilter,
};
use anyhow::{Context, Result};
use std::sync::Arc;

impl App {
    pub(super) fn details_active(&self) -> bool {
        self.details
            .as_ref()
            .is_some_and(|details| details.active())
    }

    pub(super) fn owns_picker_foreground(&self) -> bool {
        let foreground = get_foreground_window();
        foreground == self.hwnd
            || self
                .search
                .as_ref()
                .is_some_and(|search| search.active() && search.hwnd() == foreground)
            || self
                .details
                .as_ref()
                .is_some_and(|details| details.active() && details.hwnd() == foreground)
    }

    pub(super) fn show_details(&mut self) -> Result<()> {
        if !self.config.details_enable
            || get_foreground_window() != self.hwnd
            || self.details_active()
            || !self.switching.first_panel_done
        {
            return Ok(());
        }
        self.cancel_preview();
        let Some(state) = &self.switch_apps_state else {
            return Ok(());
        };
        let Some(entry) = state.apps.get(state.index) else {
            return Ok(());
        };
        let details = self
            .details
            .as_mut()
            .context("details stage=open disabled")?;
        details.open(
            entry.application.key.clone(),
            entry.windows.clone(),
            entry.key.identity,
            &self.config,
            state.monitor,
        )?;
        self.input
            .set_surface(InputSurface::Details, details.hwnd());
        self.switching.sticky = true;
        self.accessibility.hide();
        self.painter.hide();
        self.icons.cancel();
        self.switching.icon_keys.clear();
        debug!(
            "details stage=open session={} windows={}",
            self.input_session,
            entry.windows.len()
        );
        Ok(())
    }

    pub(super) fn sync_details(&mut self) -> Result<()> {
        if let Some(details) = self.details.as_mut().filter(|details| details.active()) {
            let records = self
                .switch_apps_state
                .as_ref()
                .and_then(|state| {
                    state
                        .apps
                        .iter()
                        .find(|entry| &entry.application.key == details.group())
                })
                .map_or_else(|| Arc::from([]), |entry| entry.windows.clone());
            details.refresh(records)?;
        }
        Ok(())
    }

    pub(super) fn poll_details(&mut self) -> Result<()> {
        let action = self
            .details
            .as_mut()
            .map(|details| details.poll())
            .transpose()?
            .flatten();
        match action {
            Some(DetailsAction::Cancel) => self.complete_switch(),
            Some(DetailsAction::Back) => {
                self.cancel_preview();
                if !self.owns_picker_foreground() {
                    self.complete_switch();
                    return Ok(());
                }
                self.details.as_mut().unwrap().close();
                if self
                    .switch_apps_state
                    .as_ref()
                    .is_none_or(|state| state.apps.is_empty())
                {
                    self.complete_switch();
                } else {
                    self.switching.paint_dirty = true;
                    self.switching.icon_keys.clear();
                    self.flush_panel()?;
                    debug!("details stage=back session={}", self.input_session);
                }
            }
            Some(DetailsAction::Activate(record)) => {
                self.cancel_preview();
                let identity = record.identity;
                let filter = WindowFilter::from_config(&self.config, SwitchKind::Apps)
                    .with_scope(self.switching.scope);
                if self.owns_picker_foreground()
                    && filter.allows_process(&record.process.executable)
                    && identity.is_current(&self.snapshots.lifetimes)
                    && filter.allows(identity.hwnd()).is_some()
                    && set_foreground_window(identity.hwnd(), || {
                        self.input.permits(self.input_session)
                            && identity.is_current(&self.snapshots.lifetimes)
                    })
                {
                    self.complete_switch();
                } else {
                    debug!("details stage=activation stale-or-unavailable");
                    self.request_snapshot(SwitchKind::Apps, true)?;
                }
            }
            None => {}
        }
        Ok(())
    }
}
