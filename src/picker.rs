//! Shared native list surface; search and details retain their own session models.
mod controls;
mod drawing;
mod highlight;
mod input;
mod layout;
mod list;
pub(crate) mod messages;
mod paint;
mod placement;
mod rows;
mod scroll_visibility;
mod scrollbar;
mod skin;
#[cfg(test)]
mod tests;
pub(crate) use list::label;
pub(crate) use rows::PickerRow;

use self::{controls::Controls, messages::ViewState};
use crate::{
    config::Config,
    layout::MonitorSnapshot,
    localization::Text,
    utils::{check_error, get_foreground_window, set_foreground_window, set_window_user_data},
    window_target::WindowTarget,
};
use anyhow::{ensure, Context, Result};
use once_cell::sync::OnceCell;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use windows::{
    core::{w, HSTRING},
    Win32::{
        Foundation::{COLORREF, HWND},
        Graphics::Gdi::HBRUSH,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
            WindowsAndMessaging::*,
        },
    },
};

static SEARCH_CLASS: OnceCell<u16> = OnceCell::new();
static DETAILS_CLASS: OnceCell<u16> = OnceCell::new();
pub(crate) const MAX_QUERY_UNITS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewKind {
    Search,
    Details,
}

pub(crate) struct ViewEvents {
    pub flags: u32,
    pub accept: Option<(u64, usize)>,
}

pub(crate) struct PickerWindow {
    pub hwnd: HWND,
    state: Option<Box<ViewState>>,
    controls: Option<Controls>,
    configuration: Option<Config>,
    monitor: Option<MonitorSnapshot>,
    text: Text,
}

impl PickerWindow {
    pub(crate) fn create(
        owner: HWND,
        target: Arc<WindowTarget>,
        text: Text,
        kind: ViewKind,
    ) -> Result<Self> {
        let module = unsafe { GetModuleHandleW(None) }?;
        let (class, class_name, title) = match kind {
            ViewKind::Search => (
                &SEARCH_CLASS,
                w!("WindowSwitcher.Search"),
                text.search_label(),
            ),
            ViewKind::Details => (
                &DETAILS_CLASS,
                w!("WindowSwitcher.Details"),
                text.details_label(),
            ),
        };
        class.get_or_try_init(|| -> Result<u16> {
            let class = WNDCLASSW {
                lpfnWndProc: Some(messages::window_proc),
                hInstance: module.into(),
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
                lpszClassName: class_name,
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
            alive: Cell::new(true),
            target,
            kind,
            text,
            edit: Cell::new(HWND::default()),
            list: Cell::new(HWND::default()),
            back: Cell::new(HWND::default()),
            clear: Cell::new(HWND::default()),
            dismiss: Cell::new(HWND::default()),
            visible: Cell::new(false),
            busy: Cell::new(true),
            failed: Cell::new(false),
            composing: Cell::new(false),
            suppress_enter: Cell::new(false),
            flags: Cell::new(0),
            epoch: Cell::new(0),
            accept: Cell::new(None),
            background: Cell::new(COLORREF(0)),
            foreground: Cell::new(COLORREF(0)),
            muted: Cell::new(COLORREF(0)),
            brush: Cell::new(HBRUSH::default()),
            dpi: Cell::new(96),
            query_bottom: Cell::new(0),
            visual: RefCell::new(rows::PickerVisual::default()),
            hover: Cell::new(None),
            pressed: Cell::new(None),
            hot_control: Cell::new(HWND::default()),
            paint_error: Cell::new(false),
            reset_scroll: Cell::new(true),
            scroll_drag: Cell::new(None),
            wheel_remainder: Cell::new(0),
            scroll_mode: Cell::new(crate::config::ScrollBarMode::Auto),
            scroll_hint: Cell::new(false),
            help_open: Cell::new(false),
            style_dirty: Cell::new(true),
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
                class_name,
                &HSTRING::from(title),
                WS_POPUP
                    | WS_SYSMENU
                    | WS_CLIPCHILDREN
                    | if kind == ViewKind::Details {
                        WS_CAPTION
                    } else {
                        WINDOW_STYLE(0)
                    },
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
            configuration: None,
            monitor: None,
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
    pub(crate) fn visible(&self) -> bool {
        self.state().alive.get() && self.state().visible.get()
    }
    pub(crate) fn composing(&self) -> bool {
        self.state().composing.get()
    }
    pub(crate) fn selection_available(&self) -> bool {
        self.visible() && !self.state().busy.get() && !self.composing()
    }

    pub(crate) fn position(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        self.require_live()?;
        self.controls.as_mut().unwrap().style(
            self.hwnd,
            self.state.as_deref().unwrap(),
            config,
            monitor.dpi,
        )?;
        self.configuration = Some(config.clone());
        self.monitor = Some(monitor);
        let area = monitor.available;
        let (row_height, text_height) = self.controls().metrics(self.state(), monitor.dpi);
        let (width, height) = layout::PickerLayout::target(
            config,
            monitor,
            self.state().kind,
            row_height,
            text_height,
            self.state().help_open.get(),
        );
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
        self.controls().layout(self.hwnd, self.state(), monitor.dpi)
    }

    pub(crate) fn show(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        self.state().help_open.set(false);
        self.position(config, monitor)?;
        if self.state().kind == ViewKind::Search {
            unsafe { SetWindowTextW(self.controls().edit, w!("")) }?;
        }
        self.state().flags.set(0);
        self.state().composing.set(false);
        self.state().suppress_enter.set(false);
        self.state().visible.set(true);
        self.pending()?;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
        self.focus()
    }

    pub(crate) fn focus(&self) -> Result<()> {
        self.require_live()?;
        set_foreground_window(self.hwnd, || {
            self.visible() && self.state().target.is_live()
        });
        ensure!(
            get_foreground_window() == self.hwnd,
            "search stage=focus denied"
        );
        let _ = unsafe { SetFocus(Some(self.state().focus_target())) };
        Ok(())
    }

    pub(crate) fn hide(&self) {
        if !self.state().alive.get() {
            return;
        }
        scroll_visibility::hide(self.hwnd, self.state());
        scrollbar::cancel(self.hwnd, self.state());
        self.state().reset_scroll.set(true);
        self.state().scroll_drag.set(None);
        self.state().wheel_remainder.set(0);
        self.state().visible.set(false);
        self.state().accept.set(None);
        self.state().flags.set(0);
        self.state().composing.set(false);
        self.state().suppress_enter.set(false);
        self.state().busy.set(true);
        self.state().failed.set(false);
        self.state().hover.set(None);
        self.state().pressed.set(None);
        self.state().hot_control.set(HWND::default());
        if let Ok(mut visual) = self.state().visual.try_borrow_mut() {
            visual.rows.clear();
            visual.query.clear();
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SendMessageW(self.controls().list, LB_RESETCONTENT, None, None);
            if self.state().kind == ViewKind::Search {
                if let Err(error) = SetWindowTextW(self.controls().edit, w!("")) {
                    warn!("search stage=clear-input code={:#x}", error.code().0);
                }
            }
        }
    }

    pub(crate) fn take_events(&self) -> ViewEvents {
        ViewEvents {
            flags: self.state().flags.replace(0),
            accept: self.state().accept.take(),
        }
    }
    pub(crate) fn layout(&mut self) -> Result<()> {
        self.require_live()?;
        let dpi = self.state().dpi.get();
        if let Some(config) = &self.configuration {
            self.controls.as_mut().unwrap().style(
                self.hwnd,
                self.state.as_deref().unwrap(),
                config,
                dpi,
            )?;
        }
        self.controls().layout(self.hwnd, self.state(), dpi)
    }

    pub(crate) fn fit_results(&mut self, count: usize) -> Result<()> {
        self.require_live()?;
        let (Some(mut config), Some(monitor)) = (self.configuration.clone(), self.monitor) else {
            return Ok(());
        };
        let configured = config.clone();
        config.search_visible_rows = (count.max(1) as u32).min(config.search_visible_rows);
        let (row, text) = self.controls().metrics(self.state(), monitor.dpi);
        let desired = layout::PickerLayout::target(
            &config,
            monitor,
            ViewKind::Search,
            row,
            text,
            self.state().help_open.get(),
        );
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut rect) }?;
        if (rect.right - rect.left, rect.bottom - rect.top) != desired {
            self.position(&config, monitor)?;
            self.configuration = Some(configured);
        }
        Ok(())
    }
    pub(crate) fn query(&self) -> Result<String> {
        self.require_live()?;
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
    pub(crate) fn selected(&self) -> Option<usize> {
        if !self.state().alive.get() {
            return None;
        }
        let index = unsafe { SendMessageW(self.controls().list, LB_GETCURSEL, None, None) }.0;
        (index >= 0).then_some(index as usize)
    }
    pub(crate) fn status(&self, value: &str) -> Result<()> {
        self.require_live()?;
        unsafe {
            SetWindowTextW(self.controls().status, &HSTRING::from(value))?;
            windows::Win32::UI::Accessibility::NotifyWinEvent(
                EVENT_OBJECT_NAMECHANGE,
                self.controls().status,
                OBJID_CLIENT.0,
                0,
            );
        }
        Ok(())
    }
    pub(crate) fn pending(&self) -> Result<()> {
        self.require_live()?;
        self.state().busy.set(true);
        self.state().failed.set(false);
        self.state().accept.set(None);
        self.state().pressed.set(None);
        unsafe {
            if GetFocus() == self.controls().list && GetForegroundWindow() == self.hwnd {
                let _ = SetFocus(Some(self.state().focus_target()));
            }
            let _ = EnableWindow(self.controls().list, false);
        }
        self.status(self.text.search_loading())?;
        self.controls().refresh_notice(self.state())
    }
    pub(crate) fn failure(&self) -> Result<()> {
        self.pending()?;
        self.state().failed.set(true);
        self.status(self.text.search_failed_title())?;
        self.controls().refresh_notice(self.state())
    }

    fn require_live(&self) -> Result<()> {
        ensure!(self.state().alive.get(), "picker stage=window retired");
        Ok(())
    }
}

impl Drop for PickerWindow {
    fn drop(&mut self) {
        self.state().visible.set(false);
        if !self.state().alive.get() {
            return;
        }
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
