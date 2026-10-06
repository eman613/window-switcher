//! Local commands carry the identity and epoch displayed when the user acted.
use super::{messages::ViewState, PickerWindow};
use crate::utils::window_identity::WindowIdentity;
use anyhow::Result;
use std::cell::{Cell, RefCell};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{SendMessageW, LB_GETCURSEL},
    },
};
mod view;
pub(super) use view::{create, draw, refresh};
pub(super) const CLOSE: usize = 111;
pub(super) const YES: usize = 112;
pub(super) const NO: usize = 113;
const LABEL: usize = 114;
pub(crate) type CloseEvent = (u64, usize, WindowIdentity);
#[derive(Default)]
pub(super) struct CloseState {
    enabled: Cell<bool>,
    confirm: Cell<bool>,
    targets: RefCell<Vec<(WindowIdentity, String)>>,
    pending: Cell<Option<CloseEvent>>,
    displayed: Cell<Option<CloseEvent>>,
    pressed: Cell<Option<Option<CloseEvent>>>,
    notice: RefCell<Option<String>>,
    pub(super) request: Cell<Option<CloseEvent>>,
    close: Cell<HWND>,
    yes: Cell<HWND>,
    no: Cell<HWND>,
    label: Cell<HWND>,
    tips: RefCell<Vec<Vec<u16>>>,
}
impl CloseState {
    pub(super) fn configure(&self, config: &crate::config::Config) {
        self.enabled.set(config.close_enable);
        self.confirm.set(config.close_confirm);
        self.cancel();
    }
    pub(super) fn cancel(&self) {
        self.pending.set(None);
        self.request.set(None);
        *self.notice.borrow_mut() = None;
    }
}
fn selected(state: &ViewState) -> Option<CloseEvent> {
    if !state.close.enabled.get()
        || !state.visible.get()
        || state.busy.get()
        || state.failed.get()
        || state.composing.get()
    {
        return None;
    }
    let index = unsafe { SendMessageW(state.list.get(), LB_GETCURSEL, None, None) }.0;
    if index < 0 {
        return None;
    }
    state
        .close
        .targets
        .try_borrow()
        .ok()?
        .get(index as usize)
        .map(|(id, _)| (state.epoch.get(), index as usize, *id))
}
pub(super) fn command(state: &ViewState, id: usize) {
    refresh(state);
    match id {
        CLOSE => {
            if state.close.pending.get().is_some() || state.close.request.get().is_some() {
                return;
            }
            if let Some(target) = selected(state) {
                if state.close.confirm.get() {
                    state.close.pending.set(Some(target));
                    if let Some((_, title)) = state.close.targets.borrow().get(target.1) {
                        if let Some(announcer) = state.announcer.borrow().as_ref() {
                            announcer.say(state.text.close_question(title));
                        }
                    }
                } else {
                    state.close.request.set(Some(target));
                }
            }
        }
        YES => {
            if let Some(target) = state.close.pending.take() {
                if selected(state) == Some(target) {
                    state.close.request.set(Some(target));
                }
            }
        }
        NO => {
            state.close.cancel();
        }
        _ => return,
    }
    refresh(state);
    unsafe {
        let _ = SetFocus(Some(state.focus_target()));
    }
    state
        .target
        .try_post(crate::keyboard::dispatch::WM_INPUT_READY);
}
pub(super) fn press(state: &ViewState, hwnd: HWND) {
    if hwnd == state.close.close.get() {
        state.close.pressed.set(Some(selected(state)));
    }
}
pub(super) fn native_command(state: &ViewState, id: usize) {
    if id == CLOSE {
        if let Some(pressed) = state.close.pressed.take() {
            if pressed.is_none() || pressed != selected(state) {
                refresh(state);
                return;
            }
        }
    }
    command(state, id);
}
pub(super) fn escape(state: &ViewState) -> bool {
    if state.close.pending.get().is_some() {
        command(state, NO);
        true
    } else {
        false
    }
}
pub(super) fn confirming(state: &ViewState) -> bool {
    state.close.pending.get().is_some()
}
pub(super) fn enabled(state: &ViewState) -> bool {
    state.close.enabled.get()
}
pub(super) fn keyboard_confirm(state: &ViewState, hwnd: HWND) {
    command(
        state,
        if hwnd == state.close.no.get() {
            NO
        } else {
            YES
        },
    );
}
pub(super) fn buttons(state: &ViewState) -> [HWND; 3] {
    [
        state.close.close.get(),
        state.close.yes.get(),
        state.close.no.get(),
    ]
}
impl PickerWindow {
    pub(crate) fn close_targets(&self, targets: Vec<(WindowIdentity, String)>) {
        let state = self.state();
        state.close.cancel();
        *state.close.targets.borrow_mut() = targets;
        refresh(state);
    }
    pub(crate) fn close_status(&self, message: &str) -> Result<()> {
        *self.state().close.notice.borrow_mut() = Some(message.to_owned());
        refresh(self.state());
        self.status(message)
    }
}

#[cfg(test)]
mod tests;
