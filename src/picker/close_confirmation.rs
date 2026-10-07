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
mod targeting;
mod tooltip;
mod view;
pub(super) use tooltip::help as help_tooltip;
pub(super) use tooltip::notify;
pub(super) use tooltip::set_font as set_hint_font;
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
    tooltip: Cell<HWND>,
    updating: Cell<bool>,
    help_tip: RefCell<Vec<u16>>,
    offered: Cell<Option<CloseEvent>>,
    hint_font: RefCell<Option<crate::utils::gdi::OwnedGdiObject>>,
}
impl CloseState {
    pub(super) fn configure(&self, config: &crate::config::Config) {
        self.enabled.set(config.close_enable);
        self.confirm.set(config.close_confirm);
        self.cancel();
    }
    pub(super) fn cancel(&self) {
        if self.pending.get().is_some() {
            debug!("close stage=confirmation-cancelled");
        }
        self.pending.set(None);
        self.request.set(None);
        *self.notice.borrow_mut() = None;
    }
}
fn selected(state: &ViewState) -> Option<CloseEvent> {
    let index = unsafe { SendMessageW(state.list.get(), LB_GETCURSEL, None, None) }.0;
    usize::try_from(index)
        .ok()
        .and_then(|index| target_at(state, index))
}
fn target_at(state: &ViewState, index: usize) -> Option<CloseEvent> {
    if !state.close.enabled.get()
        || !state.visible.get()
        || state.busy.get()
        || state.failed.get()
        || state.composing.get()
        || state.close.updating.get()
    {
        return None;
    }
    state
        .close
        .targets
        .try_borrow()
        .ok()?
        .get(index)
        .map(|(id, _)| (state.epoch.get(), index, *id))
}
pub(super) fn command(state: &ViewState, id: usize) {
    if state.close.updating.get() {
        return;
    }
    refresh(state);
    match id {
        CLOSE => {
            if state.close.pending.get().is_some() || state.close.request.get().is_some() {
                return;
            }
            if let Some(target) = selected(state) {
                if state.close.confirm.get() {
                    state.close.pending.set(Some(target));
                    debug!("close stage=confirmation-opened");
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
    crate::picker::repaint::row(state, selected(state).map(|target| target.1));
    state.signal(super::messages::RELAYOUT);
    unsafe {
        let _ = SetFocus(Some(state.focus_target()));
    }
    state
        .target
        .try_post(crate::keyboard::dispatch::WM_INPUT_READY);
}
pub(super) fn press(state: &ViewState, hwnd: HWND) {
    if hwnd == state.close.close.get() {
        state.close.pressed.set(Some(state.close.offered.get()));
    } else if hwnd == state.close.yes.get() {
        state.close.pressed.set(Some(selected(state)));
    }
}
pub(super) fn native_command(state: &ViewState, id: usize) {
    if id == CLOSE {
        let target = state
            .close
            .pressed
            .take()
            .unwrap_or(state.close.offered.get());
        if !targeting::select_offered(state, target) {
            refresh(state);
            return;
        }
    } else if id == YES {
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
pub(super) fn row_notice(state: &ViewState, index: usize) -> Option<String> {
    let target = selected(state).filter(|target| target.1 == index)?;
    state.close.notice.borrow().clone().or_else(|| {
        (state.close.pending.get() == Some(target)).then(|| {
            state
                .text
                .close_question(&state.close.targets.borrow()[index].1)
        })
    })
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
    pub(crate) fn update_close_targets(
        &self,
        targets: Vec<(WindowIdentity, String)>,
        update: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let state = self.state();
        // Row replacement sends synchronous native messages. Never expose a
        // new epoch with the old identity table to those callbacks.
        state.close.updating.set(true);
        let result = update();
        *state.close.targets.borrow_mut() = targets;
        state.close.updating.set(false);
        let current = selected(state);
        if result.is_err() {
            state.close.cancel();
        } else if let Some(previous) = state.close.pending.get() {
            if current.is_some_and(|target| target.2 == previous.2) {
                state.close.pending.set(current);
                state.close.displayed.set(current);
                debug!("close stage=confirmation-refresh retained=true");
            } else {
                debug!("close stage=confirmation-refresh retained=false");
                state.close.cancel();
            }
        }
        refresh(state);
        result
    }
    pub(crate) fn close_status(&self, message: &str) -> Result<()> {
        *self.state().close.notice.borrow_mut() = Some(message.to_owned());
        refresh(self.state());
        crate::picker::repaint::row(self.state(), selected(self.state()).map(|target| target.1));
        self.status(message)
    }
}

#[cfg(test)]
mod tests;
