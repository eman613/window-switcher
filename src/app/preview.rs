use super::App;
use crate::{layout::PixelRect, preview::PreviewRequest};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, IsWindowVisible},
};

impl App {
    pub(super) fn cancel_preview(&mut self) {
        if let Some(preview) = &mut self.preview {
            preview.cancel();
        }
    }

    pub(super) fn poll_preview(&mut self) {
        if self.preview.is_none() {
            return;
        }
        let request = self.preview_request();
        let remaining_bytes = (self.config.render_budget_mb as usize * 1024 * 1024)
            .saturating_sub(self.painter.reserved_bytes());
        let session = self.input_session;
        let input = &self.input;
        let target = &self.target;
        self.preview.as_mut().unwrap().poll(
            request,
            &self.config,
            &self.snapshots.lifetimes,
            remaining_bytes,
            || target.is_live() && input.permits(session),
        );
    }

    fn preview_request(&self) -> Option<PreviewRequest> {
        if self.input_session == 0 || self.config.input_paused || self.switching.finishing.is_some()
        {
            return None;
        }
        let monitor = self.switching.monitor?;
        let (source, surface, anchor) =
            if let Some(search) = self.search.as_ref().filter(|search| search.active()) {
                (
                    search.selected_window()?,
                    search.hwnd(),
                    surface_bounds(search.hwnd())?,
                )
            } else if let Some(details) = self.details.as_ref().filter(|details| details.active()) {
                (
                    details.selected_window()?,
                    details.hwnd(),
                    surface_bounds(details.hwnd())?,
                )
            } else {
                if !self.switching.first_panel_done {
                    return None;
                }
                let state = self.switch_apps_state.as_ref()?;
                let entry = state.apps.get(state.index)?;
                (entry.key.identity, self.hwnd, self.painter.layout()?.bounds)
            };
        if unsafe { GetForegroundWindow() } != surface
            || !unsafe { IsWindowVisible(surface) }.as_bool()
        {
            return None;
        }
        Some(PreviewRequest {
            source,
            surface: surface.0 as usize,
            anchor,
            monitor,
        })
    }
}

fn surface_bounds(hwnd: HWND) -> Option<PixelRect> {
    let mut rect = RECT::default();
    match unsafe { GetWindowRect(hwnd, &mut rect) } {
        Ok(()) => Some(PixelRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }),
        Err(error) => {
            debug!("preview stage=surface-bounds code={:#x}", error.code().0);
            None
        }
    }
}
