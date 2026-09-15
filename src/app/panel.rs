use super::App;
use crate::{
    keyboard::state::SwitchKind, layout::MonitorSnapshot, utils::set_foreground_window,
    window_snapshot::filter::WindowFilter,
};
use anyhow::Result;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use windows::Win32::Foundation::HWND;

impl App {
    pub(super) fn flush_panel(&mut self) -> Result<()> {
        if self.switching.finishing.is_some() || !self.input.permits(self.input_session) {
            return Ok(());
        }
        let Some(state) = &self.switch_apps_state else {
            return Ok(());
        };
        if self.switching.paint_dirty {
            self.painter
                .paint(state, || self.input.permits(self.input_session))?;
            self.switching.paint_dirty = false;
            if !self.input.permits(self.input_session) {
                return Ok(());
            }
            if !self.switching.first_panel_done {
                if let Some(started) = self.switching.first_panel_started {
                    let loaded = state
                        .apps
                        .iter()
                        .filter(|entry| entry.icon.is_some())
                        .count();
                    info!("metrics event=first_panel_frame elapsed_us={} groups={} cached_icons={loaded}", started.elapsed().as_micros(), state.apps.len());
                }
                self.switching.first_panel_done = true;
            }
            if let Some(started) = self.switching.selection_started.take() {
                info!(
                    "metrics event=selection_redraw elapsed_us={}",
                    started.elapsed().as_micros()
                );
            }
        }
        self.request_visible_icons();
        Ok(())
    }

    fn request_visible_icons(&mut self) {
        let Some(state) = &mut self.switch_apps_state else {
            return;
        };
        let Some(layout) = self.painter.layout() else {
            return;
        };
        let mut keys: Vec<_> = layout
            .items
            .iter()
            .map(|item| state.apps[item.index].key.clone())
            .collect();
        // Release off-page UI references, so an old page cannot pin the entire cache.
        let first = layout.items.first().map_or(0, |item| item.index);
        let last = layout.items.last().map_or(0, |item| item.index);
        for (index, entry) in state.apps.iter_mut().enumerate() {
            if index < first || index > last {
                entry.icon = None;
            }
        }
        let refresh_due = self.switching.last_icons.is_none_or(|when| {
            when.elapsed()
                >= Duration::from_millis(self.config.icon_failure_ttl_ms.min(30000).into())
        });
        if self.switching.icon_keys == keys && !refresh_due {
            return;
        }
        self.switching.icon_keys = keys.clone();
        // Selection loads first, while order remains stable for request deduplication.
        if let Some(selected) = keys
            .iter()
            .position(|key| key == &state.apps[state.index].key)
        {
            keys.swap(0, selected);
        }
        self.switching.icon_pending = keys.len();
        self.switching.icon_generation = self.icons.request(keys);
        self.switching.last_icons = Some(Instant::now());
    }

    pub(super) fn poll_icons(&mut self) {
        for (generation, result) in self.icons.take() {
            if generation != self.switching.icon_generation
                || !self.input.permits(self.input_session)
            {
                continue;
            }
            let Some(state) = &mut self.switch_apps_state else {
                continue;
            };
            if let Some(entry) = state.apps.iter_mut().find(|entry| entry.key == result.key) {
                let changed = entry.icon.as_ref().map(|icon| icon.revision)
                    != result.image.as_ref().map(|icon| icon.revision);
                self.remembered_icons.shift_remove(&result.key);
                if let Some(image) = &result.image {
                    while self.remembered_icons.len() >= self.config.icon_cache_limit as usize {
                        self.remembered_icons.shift_remove_index(0);
                    }
                    self.remembered_icons
                        .insert(result.key, Arc::downgrade(image));
                }
                entry.icon = result.image;
                self.switching.paint_dirty |= changed;
            }
            self.switching.icon_pending = self.switching.icon_pending.saturating_sub(1);
            if self.switching.icon_pending == 0 {
                if let Some(started) = self.switching.first_panel_started {
                    let loaded = state
                        .apps
                        .iter()
                        .filter(|entry| entry.icon.is_some())
                        .count();
                    info!(
                        "metrics event=icons_complete elapsed_us={} loaded={loaded}",
                        started.elapsed().as_micros()
                    );
                }
            }
        }
    }

    pub(super) fn click(&mut self) {
        if let Some(index) = self.painter.find_clicked_app_index() {
            if let Some(state) = &mut self.switch_apps_state {
                if index >= state.apps.len() {
                    return;
                }
                state.index = index;
                self.do_switch_app();
                self.complete_switch();
            }
        }
    }

    pub(super) fn do_switch_app(&mut self) {
        if let Some(state) = self.switch_apps_state.take() {
            if let Some(entry) = state.apps.get(state.index) {
                let identity = entry.key.identity;
                let filter = WindowFilter::from_config(&self.config, SwitchKind::Apps);
                if filter.allows_process(&entry.executable)
                    && filter.allows(identity.hwnd()).is_some()
                {
                    set_foreground_window(identity.hwnd(), || {
                        self.input.permits(self.input_session)
                            && identity.is_current(&self.snapshots.lifetimes)
                    });
                } else {
                    debug!("window stage=activation stale-or-unavailable");
                }
            }
        }
        self.painter.hide();
    }

    pub(super) fn hide_apps(&mut self) {
        self.switch_apps_state = None;
        self.icons.cancel();
        self.switching.icon_keys.clear();
        self.switching.first_panel_done = false;
        self.switching.first_panel_started = None;
        self.switching.selection_started = None;
        self.painter.hide();
    }

    pub(super) fn cancel_switch_app(&mut self) {
        self.hide_apps();
        self.snapshots.cancel();
        self.switching = Default::default();
    }

    pub(super) fn invalidate_display(&mut self) -> Result<()> {
        self.painter.invalidate();
        if self.switch_apps_state.is_none() {
            return Ok(());
        }
        let monitor = match MonitorSnapshot::capture(&self.config, HWND(self.switching.anchor as _))
        {
            Ok(monitor) => monitor,
            Err(error) => {
                self.complete_switch();
                return Err(error);
            }
        };
        self.switching.monitor = Some(monitor);
        self.switch_apps_state.as_mut().unwrap().monitor = monitor;
        self.switching.icon_keys.clear();
        self.switching.paint_dirty = true;
        if let Err(error) = self.flush_panel() {
            self.complete_switch();
            return Err(error);
        }
        Ok(())
    }
}
