use super::PickerWindow;
use anyhow::{ensure, Result};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, IsWindowEnabled, SetFocus},
            WindowsAndMessaging::*,
        },
    },
};

impl PickerWindow {
    pub(crate) fn replace(&self, labels: &[String], selected: usize, epoch: u64) -> Result<()> {
        let list = self.controls().list;
        self.state().busy.set(true);
        self.state().accept.set(None);
        self.state().hover.set(None);
        unsafe {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
        let result = (|| -> Result<()> {
            unsafe {
                SendMessageW(list, LB_RESETCONTENT, None, None);
            }
            for label in labels {
                let label = HSTRING::from(label);
                let index = unsafe {
                    SendMessageW(
                        list,
                        LB_ADDSTRING,
                        None,
                        Some(LPARAM(label.as_ptr() as isize)),
                    )
                }
                .0;
                ensure!(index >= 0, "picker stage=populate-list allocation-failed");
            }
            if !labels.is_empty() {
                let index =
                    unsafe { SendMessageW(list, LB_SETCURSEL, Some(WPARAM(selected)), None) }.0;
                ensure!(index >= 0, "picker stage=selection invalid-index");
            }
            Ok(())
        })();
        unsafe {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, true);
        }
        if let Err(error) = result {
            self.failure()?;
            return Err(error);
        }
        self.finish_rows(epoch, labels.is_empty())
    }

    pub(super) fn finish_rows(&self, epoch: u64, empty: bool) -> Result<()> {
        let list = self.controls().list;
        self.state().reset_scroll.set(false);
        self.state().epoch.set(epoch);
        self.state().accept.set(None);
        self.state().failed.set(false);
        self.state().busy.set(false);
        unsafe {
            let focused_list = GetFocus() == list;
            if IsWindowEnabled(list).as_bool() == empty {
                let _ = EnableWindow(list, !empty);
            }
            if empty && focused_list && GetForegroundWindow() == self.hwnd {
                let _ = SetFocus(Some(self.state().focus_target()));
                debug!("picker stage=empty-focus moved-to-recovery");
            }
        }
        super::scrollbar::refresh(self.state());
        self.controls().refresh_notice(self.state())
    }
}

pub(crate) fn label(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}
