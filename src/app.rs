use crate::{
    badge::BadgeStyle,
    config::{edit_config_file, watch::ConfigWatcher, Config, LoadedConfig},
    keyboard::{
        dispatch::InputDispatch,
        state::{InputAction, SwitchKind},
    },
    painter::GdiAAPainter,
    startup::Startup,
    trayicon::TrayIcon,
    utils::{
        get_app_icon, get_foreground_window, is_iconic_window, list_windows, set_foreground_window,
        window_identity::WindowIdentity,
    },
    window_target::WindowTarget,
};

use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{DestroyIcon, HICON, WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP},
    },
};

mod bootstrap;
mod feedback;
mod lifecycle;
mod navigation;
mod runtime;

pub use bootstrap::run;

pub const NAME: PCWSTR = w!("Window Switcher");
pub const WM_USER_TRAYICON: u32 = 6000;
pub const WM_USER_REGISTER_TRAYICON: u32 = 6001;
pub const WM_USER_CONFIG_CHANGED: u32 = 6002;
pub const IDM_EXIT: u32 = 1;
pub const IDM_STARTUP: u32 = 2;
pub const IDM_CONFIGURE: u32 = 3;

pub fn start(loaded: &LoadedConfig) -> Result<()> {
    let instance = crate::utils::SingleInstance::create(crate::utils::INSTANCE_NAME)?;
    anyhow::ensure!(instance.is_single(), "应用已经在运行，本次启动已取消");
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
    cached_icons: HashMap<String, HICON>,
    painter: GdiAAPainter,
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
            WM_USER_CONFIG_CHANGED => {
                self.poll_lifecycle()?;
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
            WM_LBUTTONUP => self.click(),
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
                if let Err(err) = edit_config_file() {
                    self.report_config_error(&format!("{err:#}"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn drain_input(&mut self) {
        if let Err(error) = self.poll_lifecycle() {
            self.lifecycle_error(&error);
        }
        if !self.target.is_live() {
            return;
        }
        self.poll_feedback();
        if self.diagnostics.tick() {
            self.input.log_summary();
        }
        if let Some(code) = crate::config::take_log_failure() {
            self.notify(self.text.error_title(), &self.text.log_failure(code), true);
        }
        if self.input_session != 0 && self.input_session <= self.input.revoked() {
            self.cancel_switch_app();
            self.switch_windows_state.modifier_released = true;
            self.input.acknowledge(self.input_session);
            self.input_session = 0;
        }
        for event in self.input.take() {
            if !self.input.permits(event.session) {
                self.input.acknowledge(event.session);
                continue;
            }
            self.input_session = event.session;
            let result = match event.action {
                InputAction::Cycle(SwitchKind::Apps, reverse) => {
                    self.switch_apps(reverse).and_then(|()| {
                        if !self.input.permits(event.session) {
                            return Ok(());
                        }
                        if let Some(state) = &self.switch_apps_state {
                            self.painter
                                .paint(state, || self.input.permits(event.session))?;
                        }
                        Ok(())
                    })
                }
                InputAction::Cycle(SwitchKind::Windows, reverse) => {
                    let hwnd = self
                        .switch_apps_state
                        .as_ref()
                        .and_then(|state| state.apps.get(state.index))
                        .map(|entry| entry.representative_hwnd)
                        .unwrap_or_else(get_foreground_window);
                    let result = self.switch_windows(hwnd, reverse).map(|_| ());
                    self.cancel_switch_app();
                    result
                }
                InputAction::Finish(kind) => {
                    if kind == SwitchKind::Apps {
                        self.do_switch_app();
                    }
                    self.switch_windows_state.modifier_released = true;
                    self.input.acknowledge(event.session);
                    self.input_session = 0;
                    Ok(())
                }
                InputAction::Cancel => {
                    self.cancel_switch_app();
                    self.switch_windows_state.modifier_released = true;
                    self.input.acknowledge(event.session);
                    self.input_session = 0;
                    Ok(())
                }
            };
            if let Err(err) = result {
                error!("input stage=apply error={err:#}");
                self.input.cancel(event.session);
            }
            if event.session <= self.input.revoked() {
                self.cancel_switch_app();
                self.switch_windows_state.modifier_released = true;
                self.input.acknowledge(event.session);
                self.input_session = 0;
            }
        }
    }

    fn switch_windows(&mut self, hwnd: HWND, reverse: bool) -> Result<bool> {
        let groups = list_windows(
            self.config.switch_windows_ignore_minimal,
            self.config.switch_windows_only_current_desktop(),
            self.is_admin,
        )?;
        if !self.input.permits(self.input_session) {
            return Ok(false);
        }
        let Some((module, windows)) = groups
            .iter()
            .find(|(_, windows)| windows.iter().any(|(candidate, _)| *candidate == hwnd))
        else {
            return Ok(false);
        };
        let fresh: Vec<_> = windows
            .iter()
            .filter_map(|(hwnd, _)| WindowIdentity::capture(*hwnd))
            .collect();
        if fresh.len() < 2 {
            return Ok(false);
        }
        let mut ordered = fresh.clone();
        let mut anchor = fresh[0];
        let mut index = if reverse { fresh.len() - 1 } else { 1 };
        if let Some((cached_module, cached_anchor, cached_index, cached_windows)) =
            &self.switch_windows_state.cache
        {
            if cached_module == module {
                if self.switch_windows_state.modifier_released {
                    if *cached_anchor != fresh[0] {
                        if let Some(previous) =
                            fresh.iter().position(|identity| identity == cached_anchor)
                        {
                            index = previous;
                        }
                    }
                } else if let Some((reconciled, next)) =
                    navigation::reconcile_cycle(cached_windows, *cached_index, &fresh, reverse)
                {
                    ordered = reconciled;
                    index = next;
                    if fresh.contains(cached_anchor) {
                        anchor = *cached_anchor;
                    }
                }
            }
        }
        let Some(target) = ordered
            .get(index)
            .copied()
            .filter(|identity| identity.is_current())
        else {
            return Ok(false);
        };
        if !set_foreground_window(target.hwnd, || {
            self.input.permits(self.input_session) && target.is_current()
        }) {
            return Ok(false);
        }
        self.switch_windows_state = SwitchWindowsState {
            cache: Some((module.clone(), anchor, index, ordered)),
            modifier_released: false,
        };
        Ok(true)
    }

    fn switch_apps(&mut self, reverse: bool) -> Result<()> {
        if let Some(state) = self.switch_apps_state.as_mut() {
            if let Some(index) = navigation::cycle_index(state.index, state.apps.len(), reverse) {
                state.index = index;
            }
            return Ok(());
        }
        let windows = list_windows(
            self.config.switch_apps_ignore_minimal,
            self.config.switch_apps_only_current_desktop(),
            self.is_admin,
        )?;
        let mut apps = Vec::new();
        for (module_path, hwnds) in &windows {
            if !self.input.permits(self.input_session) {
                return Ok(());
            }
            let Some(first) = hwnds.first() else {
                continue;
            };
            let module_hwnd = if is_iconic_window(first.0) {
                hwnds.last().unwrap().0
            } else {
                first.0
            };
            let Some(identity) = WindowIdentity::capture(module_hwnd) else {
                continue;
            };
            let module_hicon = self
                .cached_icons
                .entry(module_path.clone())
                .or_insert_with(|| {
                    get_app_icon(
                        &self.config.switch_apps_override_icons,
                        module_path,
                        module_hwnd,
                    )
                });
            if module_hicon.is_invalid() {
                warn!("icon stage=load unavailable");
                continue;
            }
            apps.push(AppEntry {
                icon: *module_hicon,
                representative_hwnd: module_hwnd,
                window_count: hwnds.len(),
                identity: Some(identity),
            });
        }
        if apps.is_empty() {
            return Ok(());
        }
        let index = if apps.len() == 1 {
            0
        } else if reverse {
            apps.len() - 1
        } else {
            1
        };
        self.switch_apps_state = Some(SwitchAppsState {
            apps,
            index,
            show_badge: self.config.switch_apps_show_badge,
            badge_max: self.config.switch_apps_badge_max,
            badge_style: BadgeStyle::from_config(&self.config),
        });
        Ok(())
    }

    fn click(&mut self) {
        if let Some(state) = self.switch_apps_state.as_mut() {
            if let Some(index) = self.painter.find_clicked_app_index(state) {
                state.index = index;
                self.do_switch_app();
                self.input.acknowledge(self.input_session);
                self.input_session = 0;
            }
        }
    }

    fn do_switch_app(&mut self) {
        if let Some(state) = self.switch_apps_state.take() {
            if !self.input.permits(self.input_session) {
                self.painter.unpaint(state);
                return;
            }
            if let Some(identity) = state.apps.get(state.index).and_then(|entry| entry.identity) {
                let still_candidate = list_windows(
                    self.config.switch_apps_ignore_minimal,
                    self.config.switch_apps_only_current_desktop(),
                    self.is_admin,
                )
                .map(|groups| {
                    groups
                        .values()
                        .any(|windows| windows.iter().any(|(hwnd, _)| *hwnd == identity.hwnd))
                })
                .unwrap_or(false);
                if still_candidate && self.input.permits(self.input_session) {
                    set_foreground_window(identity.hwnd, || {
                        self.input.permits(self.input_session) && identity.is_current()
                    });
                } else {
                    debug!("window stage=activation stale-or-unavailable");
                }
            }
            self.painter.unpaint(state);
        }
    }

    fn cancel_switch_app(&mut self) {
        if let Some(state) = self.switch_apps_state.take() {
            self.painter.unpaint(state);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for (_, icon) in self.cached_icons.drain() {
            if !icon.is_invalid() {
                if let Err(err) = unsafe { DestroyIcon(icon) } {
                    warn!("icon stage=release code={:#x}", err.code().0);
                }
            }
        }
    }
}

#[derive(Default)]
struct SwitchWindowsState {
    cache: Option<(String, WindowIdentity, usize, Vec<WindowIdentity>)>,
    modifier_released: bool,
}

#[derive(Debug)]
pub struct SwitchAppsState {
    pub apps: Vec<AppEntry>,
    pub index: usize,
    pub show_badge: bool,
    pub badge_max: u32,
    pub badge_style: BadgeStyle,
}

#[derive(Debug, Clone, Copy)]
pub struct AppEntry {
    pub icon: HICON,
    pub representative_hwnd: HWND,
    pub window_count: usize,
    pub(crate) identity: Option<WindowIdentity>,
}
