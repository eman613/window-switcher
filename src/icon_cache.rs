use crate::{
    app::SwitchAppsState,
    icon_loader::{IconLoadKind, IconLoadResult, IconLoader},
    metrics::StageTimer,
    utils::{app_name_fallback, get_fallback_icon, is_window_valid},
};

use indexmap::IndexMap;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{DestroyIcon, HICON},
};

const ICON_RETRY_BACKOFF: Duration = Duration::from_secs(1);
const ICON_PENDING_TIMEOUT: Duration = Duration::from_secs(5);
const ICON_PENDING_LIMIT: usize = 64;
const ICON_RETRY_LIMIT: usize = 256;

pub(crate) const MAX_SWITCH_APPS: usize = 256;

#[derive(Debug, Clone, Copy)]
struct PendingIcon {
    generation: u64,
    requested_at: Instant,
}

pub(crate) struct IconCache {
    loader: IconLoader,
    pending: HashMap<String, PendingIcon>,
    pending_names: HashMap<String, PendingIcon>,
    retryable: HashMap<String, Instant>,
    fallback_keys: HashSet<String>,
    next_generation: u64,
    icons: IndexMap<String, HICON>,
    names: HashMap<String, String>,
    limit: usize,
}

impl IconCache {
    pub(crate) fn new(hwnd: HWND, override_icons: IndexMap<String, String>, limit: usize) -> Self {
        Self {
            loader: IconLoader::new(hwnd, Arc::new(override_icons)),
            pending: HashMap::new(),
            pending_names: HashMap::new(),
            retryable: HashMap::new(),
            fallback_keys: HashSet::new(),
            next_generation: 1,
            icons: IndexMap::new(),
            names: HashMap::new(),
            limit: limit.clamp(1, MAX_SWITCH_APPS),
        }
    }

    pub(crate) fn icon_for_app(&mut self, module_path: &str, representative_hwnd: HWND) -> HICON {
        if let Some(icon) = self.take(module_path) {
            self.request(module_path, module_path, representative_hwnd);
            return icon;
        }

        let icon = get_fallback_icon();
        self.fallback_keys.insert(module_path.to_string());
        self.schedule_retry(module_path, Instant::now());
        self.insert(module_path, icon);
        self.request(module_path, module_path, representative_hwnd);
        icon
    }

    pub(crate) fn retry_visible(&mut self, state: Option<&SwitchAppsState>) {
        let requests: Vec<(String, HWND)> = state
            .map(|state| {
                state
                    .apps
                    .iter()
                    .filter(|entry| {
                        self.retryable.contains_key(&entry.module_path)
                            && is_window_valid(entry.representative_hwnd)
                    })
                    .map(|entry| (entry.module_path.clone(), entry.representative_hwnd))
                    .collect()
            })
            .unwrap_or_default();
        for (key, representative_hwnd) in requests {
            self.request(&key, &key, representative_hwnd);
        }
        if let Some(state) = state {
            let name_requests: Vec<(String, HWND)> = state
                .apps
                .iter()
                .filter(|entry| {
                    !entry.name.is_empty()
                        && !self.names.contains_key(&entry.module_path)
                        && !self.pending_names.contains_key(&entry.module_path)
                        && is_window_valid(entry.representative_hwnd)
                })
                .map(|entry| (entry.module_path.clone(), entry.representative_hwnd))
                .collect();
            for (key, representative_hwnd) in name_requests {
                self.request_name(&key, &key, representative_hwnd);
            }
        }
    }

    pub(crate) fn name_for_app(&mut self, key: &str, representative_hwnd: HWND) -> String {
        if let Some(name) = self.names.get(key) {
            return name.clone();
        }
        self.request_name(key, key, representative_hwnd);
        app_name_fallback(key)
    }

    pub(crate) fn apply_results(&mut self, state: Option<&mut SwitchAppsState>) -> bool {
        let _timer = StageTimer::new("icon_results");
        let results = self.loader.drain_results();
        if results.is_empty() {
            return false;
        }

        let mut state = state;
        let mut repaint = false;
        for IconLoadResult {
            key,
            generation,
            hicon,
            kind,
            name,
        } in results
        {
            if kind == IconLoadKind::Name {
                if self.pending_names.get(&key).map(|item| item.generation) != Some(generation) {
                    continue;
                }
                self.pending_names.remove(&key);
                if let Some(name) = name {
                    self.names.insert(key.clone(), name.clone());
                    if let Some(current_state) = state.as_deref_mut() {
                        for entry in &mut current_state.apps {
                            if entry.module_path == key && entry.name != name {
                                entry.name = name.clone();
                                repaint = true;
                            }
                        }
                    }
                }
                continue;
            }
            if self.pending.get(&key).map(|item| item.generation) != Some(generation) {
                if let Some(raw_hicon) = hicon {
                    destroy_raw_icon(raw_hicon);
                }
                debug!("discarding stale icon result key={key} generation={generation}");
                continue;
            }
            self.pending.remove(&key);

            let Some(raw_hicon) = hicon else {
                self.schedule_retry(&key, Instant::now() + ICON_RETRY_BACKOFF);
                debug!("icon loading failed; deferring retry key={key}");
                continue;
            };
            let icon = HICON(raw_hicon as _);
            if icon.is_invalid() {
                destroy_raw_icon(raw_hicon);
                self.schedule_retry(&key, Instant::now() + ICON_RETRY_BACKOFF);
                debug!("icon loader returned an invalid handle key={key}");
                continue;
            }

            self.retryable.remove(&key);
            self.fallback_keys.remove(&key);
            self.insert(&key, icon);
            if let Some(current_state) = state.as_deref_mut() {
                for entry in &mut current_state.apps {
                    if entry.module_path == key {
                        entry.icon = icon;
                        repaint = true;
                    }
                }
            }
        }
        repaint
    }

    pub(crate) fn cleanup_for_state(&mut self, state: Option<&SwitchAppsState>) {
        let active_keys: HashSet<String> = state
            .map(|state| {
                state
                    .apps
                    .iter()
                    .map(|entry| entry.module_path.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.cleanup(&active_keys);
    }

    pub(crate) fn cleanup(&mut self, active_keys: &HashSet<String>) {
        let now = Instant::now();
        let mut expired = Vec::new();
        self.pending.retain(|key, pending| {
            if !active_keys.contains(key) {
                return false;
            }
            if now.saturating_duration_since(pending.requested_at) > ICON_PENDING_TIMEOUT {
                expired.push(key.clone());
                false
            } else {
                true
            }
        });
        for key in expired {
            self.schedule_retry(&key, now);
        }
        self.retryable.retain(|key, _| active_keys.contains(key));
        self.fallback_keys
            .retain(|key| self.icons.contains_key(key));
        self.pending_names.retain(|key, pending| {
            active_keys.contains(key)
                && now.saturating_duration_since(pending.requested_at) <= ICON_PENDING_TIMEOUT
        });
        self.names.retain(|key, _| active_keys.contains(key));
    }

    pub(crate) fn trim(&mut self, state: Option<&SwitchAppsState>) {
        while self.icons.len() > self.limit {
            let candidate = self
                .icons
                .keys()
                .find(|key| {
                    !self.pending.contains_key(*key)
                        && !state
                            .map(|state| state.apps.iter().any(|entry| &entry.module_path == *key))
                            .unwrap_or(false)
                })
                .cloned();
            let Some(candidate) = candidate else {
                debug!(
                    "icon cache over limit but all entries are in use; size={}",
                    self.icons.len()
                );
                break;
            };
            if let Some((_, icon)) = self.icons.shift_remove_entry(&candidate) {
                self.fallback_keys.remove(&candidate);
                destroy_hicon(icon);
            }
        }
    }

    fn request(&mut self, key: &str, module_path: &str, representative_hwnd: HWND) {
        if self.pending.contains_key(key) {
            return;
        }
        if self.pending.len() >= ICON_PENDING_LIMIT {
            self.schedule_retry(key, Instant::now() + ICON_RETRY_BACKOFF);
            debug!("icon pending limit reached; deferring icon key={key}");
            return;
        }
        if let Some(retry_at) = self.retryable.get(key) {
            if Instant::now() < *retry_at {
                return;
            }
        } else if cached_icon_is_ready(
            self.icons
                .get(key)
                .map(|icon| !icon.is_invalid())
                .unwrap_or(false),
            self.fallback_keys.contains(key),
        ) {
            return;
        }

        let generation = self.allocate_generation();
        if self
            .loader
            .request(key, module_path, representative_hwnd, generation)
        {
            self.pending.insert(
                key.to_string(),
                PendingIcon {
                    generation,
                    requested_at: Instant::now(),
                },
            );
            self.retryable.remove(key);
        } else {
            self.schedule_retry(key, Instant::now() + ICON_RETRY_BACKOFF);
        }
    }

    fn request_name(&mut self, key: &str, module_path: &str, representative_hwnd: HWND) {
        if self.pending_names.contains_key(key) || self.names.contains_key(key) {
            return;
        }
        let generation = self.allocate_generation();
        if self
            .loader
            .request_name(key, module_path, representative_hwnd, generation)
        {
            self.pending_names.insert(
                key.to_string(),
                PendingIcon {
                    generation,
                    requested_at: Instant::now(),
                },
            );
        }
    }

    fn take(&mut self, key: &str) -> Option<HICON> {
        let icon = self.icons.shift_remove(key)?;
        self.icons.insert(key.to_string(), icon);
        Some(icon)
    }

    fn insert(&mut self, key: &str, icon: HICON) {
        if let Some(previous) = self.icons.shift_remove(key) {
            if previous.0 != icon.0 {
                destroy_hicon(previous);
            }
        }
        self.icons.insert(key.to_string(), icon);
    }

    fn schedule_retry(&mut self, key: &str, retry_at: Instant) {
        if !self.retryable.contains_key(key) && self.retryable.len() >= ICON_RETRY_LIMIT {
            if let Some(oldest) = self
                .retryable
                .iter()
                .min_by_key(|(_, retry_at)| **retry_at)
                .map(|(key, _)| key.clone())
            {
                self.retryable.remove(&oldest);
            }
        }
        self.retryable.insert(key.to_string(), retry_at);
    }

    fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        generation
    }
}

impl Drop for IconCache {
    fn drop(&mut self) {
        self.loader.stop();
        for (_, icon) in self.icons.drain(..) {
            destroy_hicon(icon);
        }
    }
}

fn destroy_hicon(icon: HICON) {
    if icon.is_invalid() {
        return;
    }
    unsafe {
        let _ = DestroyIcon(icon);
    }
}

fn destroy_raw_icon(raw_hicon: isize) {
    if raw_hicon == 0 || raw_hicon == -1 {
        return;
    }
    destroy_hicon(HICON(raw_hicon as _));
}

fn cached_icon_is_ready(has_valid_icon: bool, is_fallback: bool) -> bool {
    has_valid_icon && !is_fallback
}

#[cfg(test)]
mod tests {
    use super::cached_icon_is_ready;

    #[test]
    fn fallback_icons_remain_retryable_after_reopen() {
        assert!(!cached_icon_is_ready(true, true));
        assert!(cached_icon_is_ready(true, false));
        assert!(!cached_icon_is_ready(false, false));
    }
}
