use crate::{
    badge::BadgeStyle,
    config::{edit_config_file, watch::ConfigWatcher, Config, LoadedConfig},
    icon_cache::{CachedIcon, IconKey},
    icon_loader::IconService,
    keyboard::dispatch::InputDispatch,
    layout::MonitorSnapshot,
    painter::GdiAAPainter,
    startup::Startup,
    trayicon::TrayIcon,
    utils::window_identity::WindowIdentity,
    window_snapshot::SnapshotService,
    window_target::WindowTarget,
};
use anyhow::Result;
use indexmap::IndexMap;
use std::sync::{Arc, Weak};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Controls::WM_MOUSELEAVE,
            WindowsAndMessaging::{
                WM_CANCELMODE, WM_CAPTURECHANGED, WM_COMMAND, WM_KILLFOCUS, WM_LBUTTONDOWN,
                WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETFOCUS,
            },
        },
    },
};

mod bootstrap;
mod coordinator;
mod feedback;
mod input;
mod lifecycle;
mod navigation;
mod panel;
mod pointer;
mod runtime;
mod search;
mod switching;
mod window_cycle;

pub use bootstrap::run;

pub const NAME: PCWSTR = w!("Window Switcher");
pub const WM_USER_TRAYICON: u32 = 6000;
pub const WM_USER_REGISTER_TRAYICON: u32 = 6001;
pub const WM_USER_CONFIG_CHANGED: u32 = 6002;
pub(super) const WM_SCENE_INVALIDATED: u32 = 6012;
pub const IDM_EXIT: u32 = 1;
pub const IDM_STARTUP: u32 = 2;
pub const IDM_CONFIGURE: u32 = 3;

pub fn start(loaded: &LoadedConfig) -> Result<()> {
    let instance = crate::utils::SingleInstance::create(crate::utils::INSTANCE_NAME)?;
    anyhow::ensure!(instance.is_single(), "应用已经在运行，本次启动已取消");
    crate::layout::enable_per_monitor()?;
    runtime::run(
        loaded,
        instance,
        None,
        crate::diagnostics::Diagnostics::new(&loaded.config, std::time::Instant::now()),
    )
}

struct App {
    hwnd: HWND,
    is_admin: bool,
    trayicon: Option<TrayIcon>,
    startup: Startup,
    config: Config,
    config_watcher: Option<ConfigWatcher>,
    switch_windows_state: SwitchWindowsState,
    switch_apps_state: Option<SwitchAppsState>,
    search: Option<crate::search::SearchSession>,
    snapshots: SnapshotService,
    icons: IconService,
    remembered_icons: IndexMap<IconKey, Weak<CachedIcon>>,
    switching: coordinator::SwitchCoordinator,
    painter: GdiAAPainter,
    accessibility: crate::accessibility::Accessibility,
    target: Arc<WindowTarget>,
    input: Arc<InputDispatch>,
    input_session: u64,
    lifecycle: lifecycle::Lifecycle,
    feedback: feedback::Feedback,
    text: crate::localization::Text,
    diagnostics: crate::diagnostics::Diagnostics,
}

impl App {
    fn handle_message(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Result<()> {
        match msg {
            WM_USER_CONFIG_CHANGED => self.poll_lifecycle()?,
            WM_SCENE_INVALIDATED => self.invalidate_display()?,
            WM_SETFOCUS | WM_KILLFOCUS => {
                self.switching.paint_dirty = true;
                self.flush_panel()?;
            }
            WM_USER_TRAYICON => {
                if matches!(lparam.0 as u32, WM_LBUTTONUP | WM_RBUTTONUP) {
                    if let Some(trayicon) = self.trayicon.as_mut() {
                        if let Some(command) =
                            trayicon.show(self.startup.state, self.startup.busy(), self.text)?
                        {
                            self.handle_command(command)?;
                        }
                    }
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_CAPTURECHANGED
            | WM_CANCELMODE => self.pointer_message(msg)?,
            WM_COMMAND if (wparam.0 >> 16) & 0xffff == 0 => {
                self.handle_command(wparam.0 as u32 & 0xffff)?
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_command(&mut self, command: u32) -> Result<()> {
        match command {
            IDM_EXIT => self.request_exit(),
            IDM_STARTUP => self.startup.toggle()?,
            IDM_CONFIGURE => {
                if let Err(error) = edit_config_file() {
                    self.report_config_error(&format!("{error:#}"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default)]
struct SwitchWindowsState {
    cache: Option<(Arc<str>, WindowIdentity, usize, Vec<WindowIdentity>)>,
    modifier_released: bool,
}

#[derive(Debug)]
pub(crate) struct SwitchAppsState {
    pub(crate) apps: Vec<AppEntry>,
    pub(crate) index: usize,
    pub(crate) show_badge: bool,
    pub(crate) badge_max: u32,
    pub(crate) badge_style: BadgeStyle,
    pub(crate) monitor: MonitorSnapshot,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct AppEntry {
    pub(crate) key: IconKey,
    pub(crate) icon: Option<Arc<CachedIcon>>,
    pub(crate) window_count: usize,
    pub(crate) executable: Arc<str>,
    pub(crate) display_name: Arc<str>,
}
