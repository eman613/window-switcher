//! Transient scroll indicators share one bounded window timer.
use super::{messages::ViewState, scrollbar, ViewKind};
use crate::config::ScrollBarMode;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetParent, KillTimer, SetTimer},
};

pub(super) const TIMER: usize = 0x5356;

pub(super) fn visible(state: &ViewState) -> bool {
    match state.scroll_mode.get() {
        ScrollBarMode::Always => true,
        ScrollBarMode::Auto => state.scroll_hint.get() || state.scroll_drag.get().is_some(),
        ScrollBarMode::Hidden => false,
    }
}

pub(super) fn reveal(state: &ViewState) {
    if state.kind != ViewKind::Search
        || !state.visible.get()
        || state.scroll_mode.get() != ScrollBarMode::Auto
    {
        return;
    }
    if let Ok(parent) = unsafe { GetParent(state.list.get()) } {
        if !scrollbar::has_overflow(parent, state) {
            return;
        }
        let changed = !state.scroll_hint.replace(true);
        if unsafe { SetTimer(Some(parent), TIMER, 1200, None) } == 0 {
            warn!("search stage=scroll-indicator timer-failed; keeping indicator visible");
        }
        if changed {
            debug!("search stage=scroll-indicator visible=true");
            scrollbar::invalidate(state);
        }
    }
}

pub(super) fn hide(hwnd: HWND, state: &ViewState) {
    if state.scroll_hint.replace(false) {
        debug!("search stage=scroll-indicator visible=false");
        if let Err(error) = unsafe { KillTimer(Some(hwnd), TIMER) } {
            debug!("search stage=scroll-timer-stop code={:#x}", error.code().0);
        }
        scrollbar::invalidate(state);
    }
}

pub(super) fn tick(hwnd: HWND, state: &ViewState) {
    if state.scroll_mode.get() != ScrollBarMode::Auto
        || !state.visible.get()
        || (state.scroll_drag.get().is_none() && !scrollbar::pointer_on_track(hwnd, state))
    {
        hide(hwnd, state);
    }
}
