use super::{navigation, App, SwitchWindowsState};
use crate::{
    config::SwitchOrder,
    keyboard::state::SwitchKind,
    utils::set_foreground_window,
    window_snapshot::{filter::WindowFilter, WindowSnapshot},
};
use windows::Win32::Foundation::HWND;

impl App {
    pub(super) fn cycle_windows(
        &mut self,
        snapshot: &WindowSnapshot,
        anchor_window: usize,
        reverse: bool,
    ) {
        let Some((module, windows)) = snapshot.groups.iter().find(|(_, windows)| {
            windows
                .iter()
                .any(|record| record.identity.window == anchor_window)
        }) else {
            return;
        };
        let lifetimes = &self.snapshots.lifetimes;
        let fresh: Vec<_> = windows.iter().map(|record| record.identity).collect();
        if fresh.len() < 2 {
            return;
        }
        let mut ordered = fresh.clone();
        let mru = self.config.switch_windows_order == SwitchOrder::Mru;
        let anchor_index = if mru {
            fresh
                .iter()
                .position(|identity| identity.window == anchor_window)
                .unwrap_or(0)
        } else {
            0
        };
        let mut anchor = fresh[anchor_index];
        let mut index = navigation::cycle_index(anchor_index, fresh.len(), reverse).unwrap_or(0);
        if let Some((cached_module, cached_anchor, cached_index, cached_windows)) =
            &self.switch_windows_state.cache
        {
            if cached_module == module {
                if self.switch_windows_state.modifier_released {
                    if !mru && *cached_anchor != fresh[0] {
                        if let Some(previous) =
                            fresh.iter().position(|identity| identity == cached_anchor)
                        {
                            index = previous;
                        }
                    }
                } else if let Some((reconciled, next)) =
                    navigation::reconcile_cycle(cached_windows, *cached_index, &fresh, reverse)
                {
                    ordered = reconciled;
                    index = next;
                    if fresh.contains(cached_anchor) {
                        anchor = *cached_anchor;
                    }
                }
            }
        }
        let Some(target) = ordered.get(index).copied() else {
            return;
        };
        let filter = WindowFilter::from_config(&self.config, SwitchKind::Windows);
        if filter.allows(HWND(target.window as _)).is_none() {
            return;
        }
        if set_foreground_window(target.hwnd(), || {
            self.input.permits(self.input_session) && target.is_current(lifetimes)
        }) {
            self.switch_windows_state = SwitchWindowsState {
                cache: Some((module.clone(), anchor, index, ordered)),
                modifier_released: false,
            };
        }
    }
}
