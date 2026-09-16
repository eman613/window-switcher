use super::{layout::PreviewLayout, native::Placeholder, paint};
use crate::{
    appearance::Appearance,
    config::Config,
    localization::Text,
    utils::{
        check_error,
        gdi::{message_font, OwnedGdiObject},
        set_window_user_data,
    },
    window_target::WindowTarget,
};
use anyhow::{ensure, Context, Result};
use once_cell::sync::OnceCell;
use std::{cell::Cell, sync::Arc};
use windows::{
    core::{w, HSTRING},
    Win32::{
        Foundation::{COLORREF, HWND},
        Graphics::Gdi::{CreateSolidBrush, InvalidateRect, HBRUSH, HGDIOBJ},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
};

static PREVIEW_CLASS: OnceCell<u16> = OnceCell::new();

pub(super) struct PaintState {
    pub(super) target: Arc<WindowTarget>,
    pub(super) text: Text,
    pub(super) placeholder: Cell<Option<Placeholder>>,
    pub(super) background: Cell<HBRUSH>,
    pub(super) border: Cell<HBRUSH>,
    pub(super) foreground: Cell<COLORREF>,
    pub(super) font: Cell<HGDIOBJ>,
    pub(super) alive: Cell<bool>,
    pub(super) visible: Cell<bool>,
    pub(super) padding: Cell<i32>,
}

impl PaintState {
    pub(super) fn label(&self) -> &'static str {
        match self.placeholder.get() {
            None => self.text.preview_label(),
            Some(Placeholder::Unavailable) => self.text.preview_unavailable(),
            Some(Placeholder::Minimized) => self.text.preview_minimized(),
            Some(Placeholder::Protected) => self.text.preview_protected(),
            Some(Placeholder::Closed) => self.text.preview_closed(),
        }
    }
}

struct Resources {
    _background: OwnedGdiObject,
    _border: OwnedGdiObject,
    _font: OwnedGdiObject,
}

pub(super) struct PreviewWindow {
    pub(super) hwnd: HWND,
    state: Option<Box<PaintState>>,
    resources: Option<Resources>,
    dpi: u32,
    dirty: bool,
    presentation: Option<(PreviewLayout, Option<Placeholder>)>,
}

impl PreviewWindow {
    pub(super) fn create(owner: HWND, target: Arc<WindowTarget>, text: Text) -> Result<Self> {
        let module = unsafe { GetModuleHandleW(None) }?;
        PREVIEW_CLASS.get_or_try_init(|| -> Result<u16> {
            let class = WNDCLASSW {
                lpfnWndProc: Some(paint::window_proc),
                hInstance: module.into(),
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
                lpszClassName: w!("WindowSwitcher.Preview"),
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&class) };
            ensure!(
                atom != 0,
                "preview stage=register-class code={:#x}",
                windows::core::Error::from_win32().code().0
            );
            Ok(atom)
        })?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                w!("WindowSwitcher.Preview"),
                &HSTRING::from(text.preview_label()),
                WS_POPUP | WS_DISABLED,
                0,
                0,
                0,
                0,
                Some(owner),
                None,
                Some(module.into()),
                None,
            )
        }
        .context("preview stage=create-window")?;
        let window = Self {
            hwnd,
            state: Some(Box::new(PaintState {
                target,
                text,
                placeholder: Cell::new(None),
                background: Cell::new(HBRUSH::default()),
                border: Cell::new(HBRUSH::default()),
                foreground: Cell::new(COLORREF(0)),
                font: Cell::new(HGDIOBJ::default()),
                alive: Cell::new(true),
                visible: Cell::new(false),
                padding: Cell::new(8),
            })),
            resources: None,
            dpi: 0,
            dirty: true,
            presentation: None,
        };
        check_error(|| set_window_user_data(hwnd, window.state() as *const PaintState as isize))?;
        debug!("preview stage=window-created windows=1 workers=0");
        Ok(window)
    }

    fn state(&self) -> &PaintState {
        self.state.as_deref().unwrap()
    }

    pub(super) fn live(&self) -> bool {
        self.state().alive.get()
    }

    pub(super) fn unexpectedly_hidden(&self) -> bool {
        self.presentation.is_some() && !self.state().visible.get()
    }

    pub(super) fn prepare(
        &mut self,
        config: &Config,
        dpi: u32,
        layout: PreviewLayout,
        placeholder: Option<Placeholder>,
    ) -> Result<bool> {
        ensure!(self.live(), "preview stage=prepare destroyed-window");
        let changed =
            self.presentation != Some((layout, placeholder)) || self.dirty || dpi != self.dpi;
        if !changed {
            return Ok(false);
        }
        if self.dirty || self.dpi != dpi {
            let appearance = Appearance::capture(config);
            let background = OwnedGdiObject::new(
                HGDIOBJ(
                    unsafe {
                        CreateSolidBrush(crate::text_raster::colorref(appearance.panel.color))
                    }
                    .0,
                ),
                "preview-background",
            )?;
            let border = OwnedGdiObject::new(
                HGDIOBJ(
                    unsafe { CreateSolidBrush(crate::text_raster::colorref(appearance.border)) }.0,
                ),
                "preview-border",
            )?;
            let font = message_font(dpi)?;
            self.state().background.set(HBRUSH(background.0 .0));
            self.state().border.set(HBRUSH(border.0 .0));
            self.state().font.set(font.0);
            self.state()
                .foreground
                .set(crate::text_raster::colorref(appearance.text));
            self.resources = Some(Resources {
                _background: background,
                _border: border,
                _font: font,
            });
            self.dpi = dpi;
            self.dirty = false;
        }
        self.state().placeholder.set(placeholder);
        self.state().padding.set(layout.content.left);
        let bounds = layout.bounds;
        unsafe {
            SetWindowTextW(self.hwnd, &HSTRING::from(self.state().label()))?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                bounds.left,
                bounds.top,
                bounds.width(),
                bounds.height(),
                SWP_NOACTIVATE,
            )?;
            InvalidateRect(Some(self.hwnd), None, false).ok()?;
        }
        self.presentation = Some((layout, placeholder));
        Ok(true)
    }

    pub(super) fn show(&self) -> Result<()> {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        ensure!(
            self.live() && unsafe { IsWindowVisible(self.hwnd) }.as_bool(),
            "preview stage=show unavailable"
        );
        Ok(())
    }

    pub(super) fn hide(&mut self) {
        self.presentation = None;
        self.dirty = true;
        if self.live() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        self.hide();
        if self.live() {
            if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
                error!(
                    "preview stage=destroy-window code={:#x}; retaining native owners",
                    error.code().0
                );
                std::mem::forget(self.state.take());
                std::mem::forget(self.resources.take());
            }
        }
    }
}
