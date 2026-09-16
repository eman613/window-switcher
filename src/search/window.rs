use super::{
    controls::Controls,
    messages::{self, ViewState},
    SearchEntry, MAX_QUERY_UNITS,
};
use crate::{
    config::Config,
    layout::MonitorSnapshot,
    localization::Text,
    utils::{check_error, get_foreground_window, set_foreground_window, set_window_user_data},
    window_target::WindowTarget,
};
use anyhow::{ensure, Context, Result};
use once_cell::sync::OnceCell;
use std::{cell::Cell, sync::Arc};
use windows::{
    core::{w, HSTRING},
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, WPARAM},
        Graphics::Gdi::HBRUSH,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Accessibility::NotifyWinEvent,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::*,
        },
    },
};

static CLASS: OnceCell<u16> = OnceCell::new();
const CLASS_NAME: windows::core::PCWSTR = w!("WindowSwitcher.Search");

pub(super) struct ViewEvents {
    pub flags: u32,
    pub accept: Option<(u64, usize)>,
}

pub(super) struct SearchWindow {
    pub hwnd: HWND,
    state: Option<Box<ViewState>>,
    controls: Option<Controls>,
    dpi: u32,
    text: Text,
}

impl SearchWindow {
    pub(super) fn create(owner: HWND, target: Arc<WindowTarget>, text: Text) -> Result<Self> {
        let module = unsafe { GetModuleHandleW(None) }?;
        CLASS.get_or_try_init(|| -> Result<u16> {
            let class = WNDCLASSW {
                lpfnWndProc: Some(messages::window_proc),
                hInstance: module.into(),
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&class) };
            ensure!(
                atom != 0,
                "search stage=register-class code={:#x}",
                windows::core::Error::from_win32().code().0
            );
            Ok(atom)
        })?;
        let state = Box::new(ViewState {
            target,
            edit: Cell::new(HWND::default()),
            list: Cell::new(HWND::default()),
            visible: Cell::new(false),
            busy: Cell::new(true),
            composing: Cell::new(false),
            suppress_enter: Cell::new(false),
            flags: Cell::new(0),
            epoch: Cell::new(0),
            accept: Cell::new(None),
            background: Cell::new(COLORREF(0)),
            foreground: Cell::new(COLORREF(0)),
            brush: Cell::new(HBRUSH::default()),
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
                CLASS_NAME,
                &HSTRING::from(text.search_label()),
                WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
                0,
                0,
                640,
                440,
                Some(owner),
                None,
                Some(module.into()),
                None,
            )
        }
        .context("search stage=create-window")?;
        let mut window = Self {
            hwnd,
            state: Some(state),
            controls: None,
            dpi: 96,
            text,
        };
        check_error(|| set_window_user_data(hwnd, window.state() as *const ViewState as isize))?;
        window.controls = Some(Controls::create(hwnd, window.state(), text)?);
        Ok(window)
    }

    fn state(&self) -> &ViewState {
        self.state.as_deref().unwrap()
    }
    fn controls(&self) -> &Controls {
        self.controls.as_ref().unwrap()
    }
    pub(super) fn visible(&self) -> bool {
        self.state().visible.get()
    }
    pub(super) fn composing(&self) -> bool {
        self.state().composing.get()
    }

    pub(super) fn position(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        let area = monitor.available;
        let width = (640 * monitor.dpi / 96) as i32;
        let height = (440 * monitor.dpi / 96) as i32;
        let (width, height) = (width.min(area.width()), height.min(area.height()));
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                area.left + (area.width() - width) / 2,
                area.top + (area.height() - height) / 2,
                width,
                height,
                SWP_NOACTIVATE,
            )
        }
        .context("search stage=position")?;
        self.dpi = monitor.dpi;
        self.controls.as_mut().unwrap().style(
            self.hwnd,
            self.state.as_deref().unwrap(),
            config,
            self.dpi,
        )
    }

    pub(super) fn show(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        self.position(config, monitor)?;
        unsafe { SetWindowTextW(self.controls().edit, w!("")) }?;
        self.state().flags.set(0);
        self.state().composing.set(false);
        self.state().visible.set(true);
        self.pending()?;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
        self.focus()
    }

    pub(super) fn focus(&self) -> Result<()> {
        set_foreground_window(self.hwnd, || {
            self.visible() && self.state().target.is_live()
        });
        ensure!(
            get_foreground_window() == self.hwnd,
            "search stage=focus denied"
        );
        let _ = unsafe { SetFocus(Some(self.controls().edit)) };
        Ok(())
    }

    pub(super) fn hide(&self) {
        self.state().visible.set(false);
        self.state().accept.set(None);
        self.state().flags.set(0);
        self.state().composing.set(false);
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SendMessageW(self.controls().list, LB_RESETCONTENT, None, None);
            if let Err(error) = SetWindowTextW(self.controls().edit, w!("")) {
                warn!("search stage=clear-input code={:#x}", error.code().0);
            }
        }
    }

    pub(super) fn take_events(&self) -> ViewEvents {
        ViewEvents {
            flags: self.state().flags.replace(0),
            accept: self.state().accept.take(),
        }
    }
    pub(super) fn layout(&self) -> Result<()> {
        self.controls().layout(self.hwnd, self.dpi)
    }
    pub(super) fn query(&self) -> Result<String> {
        let length = unsafe { GetWindowTextLengthW(self.controls().edit) } as usize;
        ensure!(
            length <= MAX_QUERY_UNITS,
            "search stage=query length-limit exceeded"
        );
        let mut units = vec![0; length + 1];
        let copied = unsafe { GetWindowTextW(self.controls().edit, &mut units) } as usize;
        ensure!(copied == length, "search stage=query incomplete-read");
        Ok(String::from_utf16_lossy(&units[..copied]))
    }
    pub(super) fn selected(&self) -> Option<usize> {
        let index = unsafe { SendMessageW(self.controls().list, LB_GETCURSEL, None, None) }.0;
        (index >= 0).then_some(index as usize)
    }
    pub(super) fn status(&self, value: &str) -> Result<()> {
        unsafe {
            SetWindowTextW(self.controls().status, &HSTRING::from(value))?;
            NotifyWinEvent(
                EVENT_OBJECT_NAMECHANGE,
                self.controls().status,
                OBJID_CLIENT.0,
                0,
            );
        }
        Ok(())
    }
    pub(super) fn pending(&self) -> Result<()> {
        self.state().busy.set(true);
        self.state().accept.set(None);
        unsafe {
            let _ = EnableWindow(self.controls().list, false);
        }
        self.status(self.text.search_loading())
    }
    pub(super) fn failure(&self) -> Result<()> {
        self.pending()?;
        self.status(self.text.search_failure())
    }

    pub(super) fn replace(
        &self,
        entries: &[SearchEntry],
        total: usize,
        selected: usize,
        epoch: u64,
    ) -> Result<()> {
        let list = self.controls().list;
        self.state().busy.set(true);
        unsafe {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
        let result = (|| -> Result<()> {
            unsafe {
                SendMessageW(list, LB_RESETCONTENT, None, None);
            }
            for entry in entries {
                let label = HSTRING::from(entry.label());
                let index = unsafe {
                    SendMessageW(
                        list,
                        LB_ADDSTRING,
                        None,
                        Some(LPARAM(label.as_ptr() as isize)),
                    )
                }
                .0;
                ensure!(index >= 0, "search stage=populate-list allocation-failed");
            }
            if !entries.is_empty() {
                let index =
                    unsafe { SendMessageW(list, LB_SETCURSEL, Some(WPARAM(selected)), None) }.0;
                ensure!(index >= 0, "search stage=selection invalid-index");
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
            let _ = EnableWindow(list, !entries.is_empty());
        }
        self.status(&self.text.search_count(entries.len(), total))
    }
}

impl Drop for SearchWindow {
    fn drop(&mut self) {
        self.state().visible.set(false);
        if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
            // A live native callback must never point into freed Rust memory.
            // Retain its bounded owners on an exceptional native destroy failure.
            error!(
                "search stage=destroy-window code={:#x}; retaining native owners",
                error.code().0
            );
            std::mem::forget(self.state.take());
            std::mem::forget(self.controls.take());
        }
    }
}
