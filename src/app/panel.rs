use super::App;
use crate::{
    icon_loader::IconRequest,
    keyboard::state::{InputSurface, SwitchKind},
    utils::set_foreground_window,
    window_snapshot::filter::WindowFilter,
};
use anyhow::Result;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

impl App {
    pub(super) fn flush_panel(&mut self) -> Result<()> {
        if self.details_active()
            || self.switching.finishing.is_some()
            || !self.input.permits(self.input_session)
        {
            return Ok(());
        }
        let Some(state) = &self.switch_apps_state else {
            return Ok(());
        };
        // The input thread must observe the surface before ShowWindow transfers
        // focus. Its foreground check still gates keys until this panel is active.
        self.input.set_surface(InputSurface::Panel, self.hwnd);
        if self.switching.paint_dirty {
            self.painter
                .paint(state, || self.input.permits(self.input_session))?;
            self.switching.paint_dirty = false;
            if !self.input.permits(self.input_session) {
                return Ok(());
            }
            let accessibility_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
            if let Some(layout) = self.painter.layout() {
                self.switching.paint_dirty |=
                    !self
                        .accessibility
                        .publish(state, layout, self.input_session);
            }
            crate::diagnostics::stage_elapsed("accessibility-publish", accessibility_start);
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
        // UIA exposes every candidate, including pages that have not been painted.
        // Query those names after visible icons without allocating off-page rasters.
        let requests = keys
            .into_iter()
            .map(|key| IconRequest { key, image: true })
            .chain(
                state
                    .apps
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index < first || *index > last)
                    .map(|(_, entry)| IconRequest {
                        key: entry.key.clone(),
                        image: false,
                    }),
            )
            .collect();
        self.switching.icon_generation = self.icons.request(requests);
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
                let display_name = entry.application.name(&result.display_name);
                let mut changed = entry.display_name != display_name;
                if result.image_requested {
                    changed |= entry.icon.as_ref().map(|icon| icon.revision)
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
                }
                entry.display_name = display_name;
                self.switching.paint_dirty |= changed;
            }
            if !result.image_requested {
                continue;
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

    pub(super) fn do_switch_app(&mut self) {
        self.cancel_preview();
        self.accessibility.hide();
        if let Some(state) = self.switch_apps_state.take() {
            if let Some(entry) = state.apps.get(state.index) {
                let identity = entry.key.identity;
                let filter = WindowFilter::from_config(&self.config, SwitchKind::Apps)
                    .with_scope(self.switching.scope);
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
        self.cancel_preview();
        self.input.set_surface(InputSurface::None, self.hwnd);
        self.accessibility.hide();
        self.switch_apps_state = None;
        self.icons.cancel();
        self.switching.icon_keys.clear();
        self.switching.first_panel_done = false;
        self.switching.first_panel_started = None;
        self.switching.selection_started = None;
        self.painter.hide();
    }

    pub(super) fn cancel_switch_app(&mut self) {
        self.cancel_preview();
        if let Some(identity) = self.switching.return_focus {
            // Restore only while this panel still owns the foreground. Do not
            // steal focus back after an external application became active.
            set_foreground_window(identity.hwnd(), || {
                self.target.is_live()
                    && self.owns_picker_foreground()
                    && identity.is_current(&self.snapshots.lifetimes)
            });
        }
        if let Some(search) = self.search.as_mut() {
            search.close();
        }
        if let Some(details) = self.details.as_mut() {
            details.close();
        }
        self.hide_apps();
        self.snapshots.cancel();
        self.switching = Default::default();
    }

    pub(super) fn invalidate_display(&mut self) -> Result<()> {
        self.cancel_preview();
        self.painter.invalidate();
        let Some(previous) = self.switching.monitor else {
            return Ok(());
        };
        let monitor = match previous.refresh(&self.config) {
            Ok(monitor) => monitor,
            Err(error) => {
                self.complete_switch();
                return Err(error);
            }
        };
        self.switching.monitor = Some(monitor);
        if let Some(search) = self.search.as_mut().filter(|search| search.active()) {
            search.reposition(&self.config, monitor)?;
        }
        if let Some(details) = self.details.as_mut().filter(|details| details.active()) {
            details.reposition(&self.config, monitor)?;
        }
        if let Some(state) = self.switch_apps_state.as_mut() {
            state.monitor = monitor;
        }
        self.switching.icon_keys.clear();
        self.switching.paint_dirty = true;
        if let Err(error) = self.flush_panel() {
            self.complete_switch();
            return Err(error);
        }
        Ok(())
    }
}
