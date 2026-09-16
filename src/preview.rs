//! One delayed, nonactivating DWM preview, driven by the existing UI poll.
mod layout;
mod native;
mod paint;
mod selection;
mod window;

use crate::{
    config::Config,
    layout::{MonitorSnapshot, PixelRect},
    localization::Text,
    utils::window_identity::WindowIdentity,
    window_snapshot::lifetimes::WindowLifetimes,
    window_target::WindowTarget,
};
use anyhow::Result;
use layout::PreviewLayout;
use native::{Placeholder, SourceStatus, Thumbnail};
use selection::SelectionDelay;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use window::PreviewWindow;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetForegroundWindow, IsWindowVisible},
};

const SOURCE_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreviewRequest {
    pub(crate) source: WindowIdentity,
    pub(crate) surface: usize,
    pub(crate) anchor: PixelRect,
    pub(crate) monitor: MonitorSnapshot,
}

impl PreviewRequest {
    fn surface_current(self) -> bool {
        let surface = HWND(self.surface as _);
        unsafe { GetForegroundWindow() == surface && IsWindowVisible(surface).as_bool() }
    }
}

pub(crate) struct WindowPreview {
    owner: HWND,
    target: Arc<WindowTarget>,
    text: Text,
    selection: SelectionDelay,
    thumbnail: Thumbnail,
    window: Option<PreviewWindow>,
    retiring: bool,
    next_check: Option<Instant>,
    retry_after: Option<Instant>,
    status: Option<SourceStatus>,
}

impl WindowPreview {
    pub(crate) fn new(owner: HWND, target: Arc<WindowTarget>, text: Text) -> Self {
        Self {
            owner,
            target,
            text,
            selection: SelectionDelay::default(),
            thumbnail: Thumbnail::default(),
            window: None,
            retiring: false,
            next_check: None,
            retry_after: None,
            status: None,
        }
    }

    fn retire(&mut self) -> bool {
        if let Some(window) = &mut self.window {
            window.hide();
        }
        self.retiring = !self.thumbnail.release();
        !self.retiring
    }

    pub(crate) fn cancel(&mut self) {
        self.selection.select(None, Instant::now());
        self.retire();
        self.status = None;
        self.next_check = None;
        self.retry_after = None;
    }

    pub(crate) fn poll(
        &mut self,
        request: Option<PreviewRequest>,
        config: &Config,
        lifetimes: &WindowLifetimes,
        remaining_bytes: usize,
        permitted: impl Fn() -> bool,
    ) {
        let now = Instant::now();
        let request = request.filter(|request| permitted() && request.surface_current());
        if self.selection.select(request, now) {
            self.retire();
            self.status = None;
            self.next_check = None;
            self.retry_after = None;
        }
        if self.next_check.is_some_and(|deadline| now < deadline) {
            return;
        }
        if self.retiring {
            self.next_check = Some(now + SOURCE_CHECK_INTERVAL);
            if !self.retire() {
                return;
            }
        }
        let Some(request) = self
            .selection
            .ready(now, Duration::from_millis(config.preview_delay_ms.into()))
        else {
            return;
        };
        self.next_check = Some(now + SOURCE_CHECK_INTERVAL);
        if self.retry_after.is_some_and(|deadline| now < deadline) {
            return;
        }
        if let Err(error) = self.refresh(request, config, lifetimes, remaining_bytes, &permitted) {
            warn!("preview stage=refresh error={error:#}; using placeholder");
            self.retry_after = Some(now + FAILURE_RETRY_INTERVAL);
            if self.retire() && permitted() && request.surface_current() {
                // Preview failures do not cancel or block the actual switcher.
                if let Err(error) = self.present(
                    request,
                    config,
                    (16, 9),
                    remaining_bytes,
                    Some(Placeholder::Unavailable),
                    &permitted,
                ) {
                    warn!("preview stage=placeholder error={error:#}; hiding preview");
                    self.retire();
                }
            }
        }
    }

    fn ensure_window(&mut self) -> Result<()> {
        if self.window.as_ref().is_some_and(|window| !window.live()) {
            // Release the destination's lease before replacing its native owner.
            if !self.retire() {
                return Ok(());
            }
            self.window = None;
        }
        if self.window.is_none() {
            self.window = Some(PreviewWindow::create(
                self.owner,
                self.target.clone(),
                self.text,
            )?);
        }
        Ok(())
    }

    fn refresh(
        &mut self,
        request: PreviewRequest,
        config: &Config,
        lifetimes: &WindowLifetimes,
        remaining_bytes: usize,
        permitted: &impl Fn() -> bool,
    ) -> Result<()> {
        if self
            .window
            .as_ref()
            .is_some_and(PreviewWindow::unexpectedly_hidden)
        {
            self.retire();
            return Ok(());
        }
        let status = native::inspect(request.source, lifetimes)?;
        if self.status != Some(status) {
            debug!("preview stage=source status={status:?}");
            self.status = Some(status);
        }
        if let SourceStatus::Placeholder(placeholder) = status {
            if self.thumbnail.active() && !self.retire() {
                return Ok(());
            }
            return self.present(
                request,
                config,
                (16, 9),
                remaining_bytes,
                Some(placeholder),
                permitted,
            );
        }
        if PreviewLayout::calculate(config, request, (16, 9), remaining_bytes).is_none() {
            self.retire();
            return Ok(());
        }
        self.ensure_window()?;
        if self.retiring {
            return Ok(());
        }
        if !self.thumbnail.active() {
            if !permitted() || !request.surface_current() || !request.source.is_current(lifetimes) {
                self.cancel();
                return Ok(());
            }
            self.thumbnail
                .attach(self.window.as_ref().unwrap().hwnd, request.source.hwnd())?;
        }
        let source = self.thumbnail.size()?;
        if !request.source.is_current(lifetimes) {
            self.retire();
            return Ok(());
        }
        self.present(request, config, source, remaining_bytes, None, permitted)
    }

    fn present(
        &mut self,
        request: PreviewRequest,
        config: &Config,
        source: (i32, i32),
        remaining_bytes: usize,
        placeholder: Option<Placeholder>,
        permitted: &impl Fn() -> bool,
    ) -> Result<()> {
        let Some(layout) = PreviewLayout::calculate(config, request, source, remaining_bytes)
        else {
            self.retire();
            return Ok(());
        };
        self.ensure_window()?;
        if self.retiring {
            return Ok(());
        }
        let window = self.window.as_mut().unwrap();
        let changed = window.prepare(config, request.monitor.dpi, layout, placeholder)?;
        if changed && placeholder.is_none() {
            self.thumbnail.show(layout)?;
        }
        if !permitted() || !request.surface_current() {
            self.cancel();
            return Ok(());
        }
        window.show()?;
        if changed {
            debug!("preview stage=present kind={} width={} height={} output_bytes={} budget_bytes={remaining_bytes}",
                if placeholder.is_some() { "placeholder" } else { "live" }, layout.bounds.width(), layout.bounds.height(), layout.output_bytes);
        }
        Ok(())
    }
}

impl Drop for WindowPreview {
    fn drop(&mut self) {
        if !self.retire() {
            // On an exceptional native release failure keep one hidden destination
            // and its callback owners until process teardown. Never dangle them.
            std::mem::forget(self.window.take());
        }
    }
}

#[cfg(test)]
fn fixture(window: usize) -> PreviewRequest {
    let screen = PixelRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    PreviewRequest {
        source: WindowIdentity::fixture(window),
        surface: 10,
        anchor: PixelRect {
            left: 640,
            top: 320,
            right: 1280,
            bottom: 760,
        },
        monitor: MonitorSnapshot {
            identity: 1,
            screen,
            available: screen,
            dpi: 96,
        },
    }
}
