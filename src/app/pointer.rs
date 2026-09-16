use super::App;
use crate::{accessibility::ActionKind, icon_cache::IconKey};
use anyhow::Result;
use windows::Win32::UI::{
    Controls::WM_MOUSELEAVE,
    Input::KeyboardAndMouse::{
        GetCapture, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    },
    WindowsAndMessaging::{
        WM_CANCELMODE, WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    },
};

impl App {
    fn pointer_key(&self) -> Option<IconKey> {
        if self.details_active() || !self.input.permits(self.input_session) {
            return None;
        }
        let index = self.painter.find_clicked_app_index()?;
        self.switch_apps_state
            .as_ref()?
            .apps
            .get(index)
            .map(|entry| entry.key.clone())
    }

    pub(super) fn pointer_message(&mut self, message: u32) -> Result<()> {
        match message {
            WM_MOUSEMOVE => {
                let key = self.pointer_key();
                if key.is_some() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        ..Default::default()
                    };
                    if let Err(error) = unsafe { TrackMouseEvent(&mut track) } {
                        debug!("pointer stage=track-leave code={:#x}", error.code().0);
                    }
                }
                self.switching.paint_dirty |= self.painter.hover(key);
            }
            WM_LBUTTONDOWN => {
                let key = self.pointer_key();
                if key.is_some() {
                    unsafe { SetCapture(self.hwnd) };
                    if unsafe { GetCapture() } != self.hwnd {
                        return Ok(());
                    }
                }
                self.painter.hover(key.clone());
                self.painter.press(key);
                self.switching.paint_dirty = true;
            }
            WM_LBUTTONUP => {
                let key = self.pointer_key();
                let pressed = self.painter.release();
                self.release_capture();
                self.painter.hover(key.clone());
                self.switching.paint_dirty = true;
                if let Some(key) = key.filter(|key| Some(key) == pressed.as_ref()) {
                    self.activate_accessible(&key, ActionKind::Invoke);
                }
            }
            WM_MOUSELEAVE => self.switching.paint_dirty |= self.painter.hover(None),
            WM_CAPTURECHANGED | WM_CANCELMODE => {
                self.painter.release();
                self.painter.hover(None);
                self.switching.paint_dirty = true;
                if message == WM_CANCELMODE {
                    self.release_capture();
                }
            }
            _ => {}
        }
        self.flush_panel()
    }

    fn release_capture(&self) {
        if unsafe { GetCapture() } == self.hwnd {
            if let Err(error) = unsafe { ReleaseCapture() } {
                debug!("pointer stage=release-capture code={:#x}", error.code().0);
            }
        }
    }

    pub(super) fn poll_accessibility(&mut self) {
        for action in self.accessibility.take() {
            if action.session == self.input_session {
                self.activate_accessible(&action.key, action.kind);
            }
        }
    }

    fn activate_accessible(&mut self, key: &IconKey, kind: ActionKind) {
        if self.details_active()
            || !self.input.permits(self.input_session)
            || !key.identity.is_current(&self.snapshots.lifetimes)
        {
            return;
        }
        let Some(state) = &mut self.switch_apps_state else {
            return;
        };
        let Some(index) = state.apps.iter().position(|entry| &entry.key == key) else {
            return;
        };
        state.index = index;
        self.switching.paint_dirty = true;
        match kind {
            ActionKind::Select => {
                crate::utils::focus_window(self.hwnd, || self.input.permits(self.input_session));
            }
            ActionKind::Invoke => {
                self.do_switch_app();
                self.complete_switch();
            }
        }
    }
}
