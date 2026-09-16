use super::{coordinator::PendingSnapshot, navigation, App, AppEntry, SwitchAppsState};
use crate::{
    badge::BadgeStyle, icon_cache::IconKey, keyboard::state::SwitchKind, layout::MonitorSnapshot,
    utils::get_foreground_window, window_snapshot::WindowSnapshot,
};
use anyhow::{ensure, Result};
use indexmap::IndexMap;
use std::{
    sync::Weak,
    time::{Duration, Instant},
};

impl App {
    fn request_snapshot(&mut self, kind: SwitchKind, refresh: bool) -> Result<()> {
        ensure!(
            self.snapshots.healthy(),
            "snapshot stage=request worker-unavailable"
        );
        let anchor = self
            .switch_apps_state
            .as_ref()
            .and_then(|state| state.apps.get(state.index))
            .map_or_else(
                || get_foreground_window().0 as usize,
                |entry| entry.key.identity.window,
            );
        if kind == SwitchKind::Windows {
            self.hide_apps();
        }
        let generation = self.snapshots.request(kind);
        self.switching.pending = Some(PendingSnapshot {
            generation,
            kind,
            anchor,
            refresh,
            started: Instant::now(),
        });
        Ok(())
    }

    pub(super) fn pump_switches(&mut self) -> Result<()> {
        while self.input.permits(self.input_session) {
            let Some(cycle) = self.switching.actions.front().copied() else {
                break;
            };
            let (kind, reverse) = (cycle.kind, cycle.reverse);
            if kind == SwitchKind::Apps {
                if !self.switching.first_panel_done && self.switching.first_panel_started.is_none()
                {
                    self.switching.first_panel_started = cycle.received;
                }
                if let Some(state) = self.switch_apps_state.as_mut() {
                    if self.switching.first_panel_done && self.switching.selection_started.is_none()
                    {
                        self.switching.selection_started = cycle.received;
                    }
                    state.index = navigation::cycle_index(state.index, state.apps.len(), reverse)
                        .unwrap_or(0);
                    self.switching.actions.pop_front();
                    self.switching.paint_dirty = true;
                    continue;
                }
                if self.switching.monitor.is_none() {
                    self.switching.monitor = Some(MonitorSnapshot::capture(
                        &self.config,
                        get_foreground_window(),
                    )?);
                }
            }
            if self
                .switching
                .pending
                .as_ref()
                .is_none_or(|pending| pending.refresh)
            {
                self.request_snapshot(kind, false)?;
            }
            break;
        }
        if self.switching.can_finish() {
            if self.switching.finishing == Some(SwitchKind::Apps) {
                self.do_switch_app();
            }
            self.complete_switch();
        }
        Ok(())
    }

    pub(super) fn poll_switching(&mut self) -> Result<()> {
        ensure!(
            !self.switching.timed_out(),
            "snapshot stage=request safety-timeout"
        );
        ensure!(
            self.snapshots.healthy(),
            "snapshot stage=worker unavailable"
        );
        for (generation, result) in self.snapshots.take() {
            if !self.input.permits(self.input_session)
                || self
                    .switching
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.generation != generation)
            {
                continue;
            }
            let pending = self.switching.pending.take().unwrap();
            let snapshot = result?;
            self.switching.last_snapshot = Some(Instant::now());
            match pending.kind {
                SwitchKind::Apps => self.apply_app_snapshot(snapshot)?,
                SwitchKind::Windows => {
                    while self
                        .switching
                        .actions
                        .front()
                        .is_some_and(|cycle| cycle.kind == SwitchKind::Windows)
                    {
                        let cycle = self.switching.actions.pop_front().unwrap();
                        self.cycle_windows(&snapshot, pending.anchor, cycle.reverse);
                    }
                }
            }
        }
        self.poll_icons();
        if let Some(state) = &mut self.switch_apps_state {
            let now = Instant::now();
            for entry in &mut state.apps {
                if entry.icon.as_ref().is_some_and(|icon| icon.expires <= now) {
                    entry.icon = None;
                    self.switching.paint_dirty = true;
                    self.switching.icon_keys.clear();
                }
            }
        }
        if self.switching.pending.is_none() && self.switching.finishing.is_none() {
            if let Some(state) = &self.switch_apps_state {
                let elapsed = self
                    .switching
                    .last_snapshot
                    .map_or(Duration::MAX, |started| started.elapsed());
                let changed = state.revision != self.snapshots.lifetimes.revision();
                if (changed && elapsed >= Duration::from_millis(100))
                    || elapsed >= Duration::from_millis(self.config.metadata_ttl_ms.into())
                {
                    self.request_snapshot(SwitchKind::Apps, true)?;
                }
            }
        }
        Ok(())
    }

    fn apply_app_snapshot(&mut self, snapshot: WindowSnapshot) -> Result<()> {
        let old = self.switch_apps_state.take();
        let selected = old
            .as_ref()
            .and_then(|state| state.apps.get(state.index))
            .map(|entry| entry.key.group.clone());
        let old_index = old.as_ref().map_or(0, |state| state.index);
        let old_icons: IndexMap<_, _> = old
            .as_ref()
            .into_iter()
            .flat_map(|state| &state.apps)
            .filter_map(|entry| {
                entry
                    .icon
                    .as_ref()
                    .map(|icon| (entry.key.clone(), icon.clone()))
            })
            .collect();
        let mut groups = snapshot.groups;
        let mut ordered = Vec::with_capacity(groups.len());
        if let Some(old) = &old {
            for entry in &old.apps {
                if let Some(windows) = groups.shift_remove(&entry.key.group) {
                    ordered.push((entry.key.group.clone(), windows));
                }
            }
        }
        ordered.extend(groups);
        let mut apps = Vec::with_capacity(ordered.len());
        for (group, windows) in ordered {
            let Some(first) = windows.first() else {
                continue;
            };
            let record = if first.minimized {
                windows.last().unwrap()
            } else {
                first
            };
            let key = IconKey {
                group,
                identity: record.identity,
            };
            let icon = old_icons
                .get(&key)
                .cloned()
                .or_else(|| self.remembered_icons.get(&key).and_then(Weak::upgrade))
                .filter(|icon| icon.expires > Instant::now());
            let display_name = old
                .as_ref()
                .and_then(|state| state.apps.iter().find(|entry| entry.key.group == key.group))
                .map_or_else(
                    || record.process.executable.clone(),
                    |entry| entry.display_name.clone(),
                );
            apps.push(AppEntry {
                key,
                icon,
                window_count: windows.len(),
                executable: record.process.executable.clone(),
                display_name,
            });
        }
        if apps.is_empty() {
            self.complete_switch();
            return Ok(());
        }
        let index = selected
            .and_then(|group| apps.iter().position(|entry| entry.key.group == group))
            .unwrap_or(old_index.min(apps.len() - 1));
        let monitor = self
            .switching
            .monitor
            .context("layout stage=session monitor missing")?;
        self.switch_apps_state = Some(SwitchAppsState {
            apps,
            index,
            monitor,
            revision: snapshot.revision,
            show_badge: self.config.switch_apps_show_badge,
            badge_max: self.config.switch_apps_badge_max,
            badge_style: BadgeStyle::from_config(&self.config),
        });
        self.switching.icon_keys.clear();
        self.switching.paint_dirty = true;
        Ok(())
    }
}

use anyhow::Context;
