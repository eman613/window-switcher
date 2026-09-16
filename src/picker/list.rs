use super::PickerWindow;
use anyhow::{ensure, Result};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
            WindowsAndMessaging::*,
        },
    },
};

impl PickerWindow {
    pub(crate) fn replace(&self, labels: &[String], selected: usize, epoch: u64) -> Result<()> {
        let list = self.controls().list;
        self.state().busy.set(true);
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
        result?;
        self.state().epoch.set(epoch);
        self.state().accept.set(None);
        self.state().busy.set(false);
        unsafe {
            let focused_list = GetFocus() == list;
            let _ = EnableWindow(list, !labels.is_empty());
            if labels.is_empty() && focused_list && GetForegroundWindow() == self.hwnd {
                let _ = SetFocus(Some(self.state().focus_target()));
                debug!("picker stage=empty-focus moved-to-recovery");
            }
        }
        Ok(())
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
