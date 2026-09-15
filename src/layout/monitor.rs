use anyhow::{Context, Result};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
    },
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::GetCursorPos,
    },
};

use super::PixelRect;
use crate::config::{Config, MonitorPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MonitorSnapshot {
    pub(crate) identity: usize,
    pub(crate) screen: PixelRect,
    pub(crate) available: PixelRect,
    pub(crate) dpi: u32,
}

impl MonitorSnapshot {
    pub(crate) fn capture(config: &Config, foreground: HWND) -> Result<Self> {
        let monitor = unsafe {
            match config.monitor {
                MonitorPolicy::Cursor => {
                    let mut cursor = POINT::default();
                    GetCursorPos(&mut cursor).context("layout stage=cursor")?;
                    MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
                }
                MonitorPolicy::Foreground => {
                    MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
                }
                MonitorPolicy::Primary => {
                    MonitorFromWindow(HWND::default(), MONITOR_DEFAULTTOPRIMARY)
                }
            }
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(monitor, &mut info) }
            .ok()
            .context("layout stage=monitor-info")?;
        Ok(Self {
            identity: monitor.0 as usize,
            screen: from_native(info.rcMonitor),
            available: from_native(if config.use_work_area {
                info.rcWork
            } else {
                info.rcMonitor
            }),
            dpi: monitor_dpi(monitor)?,
        })
    }
}

pub(crate) fn window_monitor_dpi(hwnd: HWND) -> Result<u32> {
    monitor_dpi(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) })
}

fn monitor_dpi(monitor: HMONITOR) -> Result<u32> {
    let (mut x, mut y) = (0, 0);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) }
        .context("layout stage=monitor-dpi")?;
    anyhow::ensure!(
        x == y && (48..=960).contains(&x),
        "layout stage=monitor-dpi invalid"
    );
    Ok(x)
}

fn from_native(rect: RECT) -> PixelRect {
    PixelRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub(crate) fn enable_per_monitor() -> Result<()> {
    use windows::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        if !AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
        {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                .context("layout stage=dpi-awareness")?;
        }
    }
    Ok(())
}
