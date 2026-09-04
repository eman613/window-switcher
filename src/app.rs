use crate::config::{edit_config_file, Config};
use crate::foreground::ForegroundWatcher;
use crate::icon_cache::{IconCache, MAX_SWITCH_APPS};
use crate::icon_loader::WM_USER_ICON_READY;
use crate::keyboard::{drain_keyboard_messages, KeyboardListener};
use crate::metrics::StageTimer;
use crate::painter::GdiAAPainter;
use crate::startup::Startup;
use crate::trayicon::TrayIcon;
use crate::utils::{
    check_error, get_foreground_window, get_window_user_data, is_iconic_window,
    is_running_as_admin, is_window_valid, list_windows_with_cache, set_foreground_window,
    set_window_user_data, ProcessMetadataCache,
};

use anyhow::{anyhow, Result};
use std::collections::HashSet;
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        GetWindowLongPtrW, KillTimer, LoadCursorW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SetTimer, SetWindowLongPtrW, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, CW_USEDEFAULT, GWL_STYLE, HICON, HTCLIENT, IDC_ARROW, MSG, WINDOW_STYLE,
        WM_COMMAND, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_LBUTTONUP, WM_NCHITTEST,
        WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_EX_LAYERED,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    },
};

pub const NAME: PCWSTR = w!("Window Switcher");
pub const WM_USER_TRAYICON: u32 = 6000;
pub const WM_USER_SWITCH_APPS: u32 = 6010;
pub const WM_USER_SWITCH_APPS_DONE: u32 = 6011;
pub const WM_USER_SWITCH_APPS_CANCEL: u32 = 6012;
pub const WM_USER_SWITCH_WINDOWS: u32 = 6020;
pub const WM_USER_SWITCH_WINDOWS_DONE: u32 = 6021;
pub const WM_USER_KEYBOARD_QUEUE: u32 = 6030;
pub const IDM_EXIT: u32 = 1;
pub const IDM_STARTUP: u32 = 2;
pub const IDM_CONFIGURE: u32 = 3;

const TRAY_RETRY_TIMER_ID: usize = 1;
const TRAY_RETRY_DELAY_MS: u32 = 3_000;

pub fn start(config: &Config) -> Result<()> {
    crate::metrics::mark_logging_ready();
    info!("start config={config:?}");
    App::start(config)
}

/// Listen to this message to recreate the tray icon since the taskbar has been recreated.
static mut WM_TASKBARCREATED: u32 = 0;

pub struct App {
    hwnd: HWND,
    is_admin: bool,
    trayicon: Option<TrayIcon>,
    startup: Startup,
    config: Config,
    switch_windows_state: SwitchWindowsState,
    switch_apps_state: Option<SwitchAppsState>,
    process_metadata: ProcessMetadataCache,
    icon_cache: IconCache,
    tray_retry_pending: bool,
    painter: GdiAAPainter,
}

impl App {
    pub fn start(config: &Config) -> Result<()> {
        let hwnd = Self::create_window()?;
        let result = Self::run(hwnd, config);
        if result.is_err() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        result
    }

    fn run(hwnd: HWND, config: &Config) -> Result<()> {
        let painter = GdiAAPainter::new(hwnd, &config.appearance)?;

        let foreground_watcher = ForegroundWatcher::init(&config.switch_windows_blacklist)?;
        let keyboard_listener = KeyboardListener::init(hwnd, &config.to_hotkeys())?;

        let trayicon = match config.trayicon {
            true => Some(TrayIcon::create()?),
            false => None,
        };

        let is_admin = is_running_as_admin()?;
        debug!("is_admin {is_admin}");

        let startup = Startup::init(is_admin)?;
        let icon_cache = IconCache::new(hwnd, config.switch_apps_override_icons.clone());

        let mut app = Box::new(App {
            hwnd,
            is_admin,
            trayicon,
            startup,
            config: config.clone(),
            switch_windows_state: SwitchWindowsState {
                cache: None,
                modifier_released: true,
            },
            switch_apps_state: None,
            process_metadata: Default::default(),
            icon_cache,
            tray_retry_pending: false,
            painter,
        });

        app.set_trayicon();
        install_app(hwnd, app)?;

        let eventloop_result = Self::eventloop();
        drop(keyboard_listener);
        drop(foreground_watcher);

        let cleanup_result = take_app(hwnd).map(drop);
        match (eventloop_result, cleanup_result) {
            (Err(event_err), Err(cleanup_err)) => Err(anyhow!(
                "Message loop failed: {event_err}; app cleanup failed: {cleanup_err}"
            )),
            (Err(event_err), Ok(())) => Err(event_err),
            (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    fn eventloop() -> Result<()> {
        let mut message = MSG::default();
        loop {
            let ret = unsafe { GetMessageW(&mut message, None, 0, 0) };
            match ret.0 {
                -1 => {
                    unsafe { GetLastError() }.ok()?;
                }
                0 => break,
                _ => unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                },
            }
        }

        Ok(())
    }

    fn create_window() -> Result<HWND> {
        unsafe { WM_TASKBARCREATED = RegisterWindowMessageW(w!("TaskbarCreated")) };

        let hinstance = unsafe { GetModuleHandleW(None) }
            .map_err(|err| anyhow!("Failed to get current module handle, {err}"))?;

        let hcursor = unsafe { LoadCursorW(None, IDC_ARROW) }
            .map_err(|err| anyhow!("Failed to load arrow cursor, {err}"))?;

        let window_class = WNDCLASSW {
            hCursor: hcursor,
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: NAME,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(App::window_proc),
            ..Default::default()
        };

        let atom = check_error(|| unsafe { RegisterClassW(&window_class) })
            .map_err(|err| anyhow!("Failed to register class, {err}"))?;

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(atom as _),
                NAME,
                WINDOW_STYLE(0),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance.into()),
                None,
            )
        }
        .map_err(|err| anyhow!("Failed to create windows, {err}"))?;

        // hide caption
        let mut style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
        style &= !WS_CAPTION.0;
        unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style as _) };

        Ok(hwnd)
    }

    fn set_trayicon(&mut self) {
        if let Some(trayicon) = self.trayicon.as_mut() {
            match trayicon.register(self.hwnd) {
                Ok(()) => {
                    self.cancel_tray_retry();
                    info!("trayicon registered");
                }
                Err(err) => {
                    if !trayicon.exist() && !self.tray_retry_pending {
                        error!("{err}, retrying in 3 seconds");
                        let timer = unsafe {
                            SetTimer(
                                Some(self.hwnd),
                                TRAY_RETRY_TIMER_ID,
                                TRAY_RETRY_DELAY_MS,
                                None,
                            )
                        };
                        if timer == 0 {
                            error!("failed to schedule tray icon retry");
                        } else {
                            self.tray_retry_pending = true;
                        }
                    }
                }
            }
        }
    }

    fn cancel_tray_retry(&mut self) {
        if !self.tray_retry_pending {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TRAY_RETRY_TIMER_ID);
        }
        self.tray_retry_pending = false;
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match Self::handle_message(hwnd, msg, wparam, lparam) {
            Ok(ret) => ret,
            Err(err) => {
                error!("{err}");
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }

    fn handle_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Result<LRESULT> {
        match msg {
            WM_USER_ICON_READY => {
                debug!("message WM_USER_ICON_READY");
                with_app(hwnd, |app| {
                    app.apply_icon_results();
                    Ok(())
                })?;
                return Ok(LRESULT(0));
            }
            WM_USER_KEYBOARD_QUEUE => {
                for message in drain_keyboard_messages() {
                    if let Err(err) =
                        Self::handle_message(hwnd, message.msg, message.wparam, message.lparam)
                    {
                        error!("queued keyboard message {} failed: {err}", message.msg);
                    }
                }
                return Ok(LRESULT(0));
            }
            WM_USER_TRAYICON => {
                with_app(hwnd, |app| {
                    if let Some(trayicon) = app.trayicon.as_mut() {
                        let keycode = lparam.0 as u32;
                        if keycode == WM_LBUTTONUP || keycode == WM_RBUTTONUP {
                            trayicon.show(app.startup.is_enable)?;
                        }
                    }
                    Ok(())
                })?;
                return Ok(LRESULT(0));
            }
            WM_USER_SWITCH_APPS => {
                debug!("message WM_USER_SWITCH_APPS");
                let reverse = lparam.0 == 1;
                with_app(hwnd, |app| {
                    app.switch_apps(reverse)?;
                    if let Some(state) = &app.switch_apps_state {
                        app.painter.paint(state);
                    }
                    Ok(())
                })?;
            }
            WM_USER_SWITCH_APPS_DONE => {
                debug!("message WM_USER_SWITCH_APPS_DONE");
                with_app(hwnd, |app| {
                    app.do_switch_app();
                    Ok(())
                })?;
            }
            WM_USER_SWITCH_APPS_CANCEL => {
                debug!("message WM_USER_SWITCH_APPS_CANCEL");
                with_app(hwnd, |app| {
                    app.cancel_switch_app();
                    Ok(())
                })?;
            }
            WM_USER_SWITCH_WINDOWS => {
                debug!("message WM_USER_SWITCH_WINDOWS");
                let reverse = lparam.0 == 1;
                with_app(hwnd, |app| {
                    let target_hwnd = app
                        .switch_apps_state
                        .as_ref()
                        .and_then(|state| {
                            state
                                .apps
                                .get(state.index)
                                .map(|entry| entry.representative_hwnd)
                        })
                        .filter(|window| is_window_valid(*window))
                        .unwrap_or_else(get_foreground_window);
                    app.switch_windows(target_hwnd, reverse)?;
                    app.cancel_switch_app();
                    Ok(())
                })?;
            }
            WM_USER_SWITCH_WINDOWS_DONE => {
                debug!("message WM_USER_SWITCH_WINDOWS_DONE");
                with_app(hwnd, |app| {
                    app.switch_windows_state.modifier_released = true;
                    Ok(())
                })?;
            }
            WM_NCHITTEST => {
                return Ok(LRESULT(HTCLIENT as _));
            }
            WM_LBUTTONUP => {
                with_app(hwnd, |app| {
                    app.click();
                    Ok(())
                })?;
            }
            WM_COMMAND => {
                let value = wparam.0 as u32;
                let kind = ((value >> 16) & 0xffff) as u16;
                let id = value & 0xffff;
                if kind == 0 {
                    match id {
                        IDM_EXIT => unsafe { PostQuitMessage(0) },
                        IDM_STARTUP => {
                            with_app(hwnd, |app| app.startup.toggle())?;
                        }
                        IDM_CONFIGURE => {
                            if let Err(err) = edit_config_file() {
                                alert!("{err}");
                            }
                        }
                        _ => {}
                    }
                }
            }
            WM_ERASEBKGND => {
                return Ok(LRESULT(0));
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                let environment_changed = msg == WM_SETTINGCHANGE;
                with_app(hwnd, |app| {
                    if environment_changed {
                        app.painter.invalidate_environment();
                    } else {
                        app.painter.invalidate_layout();
                    }
                    if let Some(state) = app.switch_apps_state.as_ref() {
                        app.painter.paint(state);
                    }
                    Ok(())
                })?;
                return Ok(LRESULT(0));
            }
            WM_TIMER if wparam.0 == TRAY_RETRY_TIMER_ID => {
                with_app(hwnd, |app| {
                    app.cancel_tray_retry();
                    app.set_trayicon();
                    Ok(())
                })?;
                return Ok(LRESULT(0));
            }
            _ if unsafe { msg == WM_TASKBARCREATED } => {
                with_app(hwnd, |app| {
                    app.set_trayicon();
                    Ok(())
                })?;
            }
            _ => {}
        }
        Ok(unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
    }

    fn switch_windows(&mut self, hwnd: HWND, reverse: bool) -> Result<bool> {
        let _timer = StageTimer::new("switch_windows");
        if !is_window_valid(hwnd) {
            self.switch_windows_state.cache = None;
            return Ok(false);
        }
        if !self.switch_windows_state.modifier_released && self.switch_cached_window(reverse) {
            return Ok(true);
        }

        let windows = list_windows_with_cache(
            self.config.switch_windows_ignore_minimal,
            self.config.switch_windows_only_current_desktop(),
            self.is_admin,
            &mut self.process_metadata,
        )?;
        debug!(
            "switch windows: hwnd:{hwnd:?} reverse:{reverse} state:{:?}",
            self.switch_windows_state
        );
        let module_path = match windows
            .iter()
            .find(|(_, v)| v.iter().any(|(id, _)| *id == hwnd))
            .map(|(k, _)| k.clone())
        {
            Some(v) => v,
            None => return Ok(false),
        };
        match windows.get(&module_path) {
            None => Ok(false),
            Some(windows) => {
                let current_windows: Vec<HWND> = windows
                    .iter()
                    .map(|(window, _)| *window)
                    .filter(|window| is_window_valid(*window))
                    .collect();
                let windows_len = current_windows.len();
                if windows_len < 2 {
                    self.switch_windows_state.cache = None;
                    return Ok(false);
                }
                let current_id = current_windows[0];
                let mut index = 1.min(windows_len - 1);
                let mut state_id = current_id;
                let mut state_windows = current_windows.clone();
                if let Some(cache) = self.switch_windows_state.cache.as_ref() {
                    if cache.module_path == module_path {
                        if self.switch_windows_state.modifier_released {
                            if cache.active_hwnd != current_id {
                                if let Some(i) = current_windows
                                    .iter()
                                    .position(|window| *window == cache.active_hwnd)
                                {
                                    index = i;
                                }
                            }
                        } else {
                            state_id = if current_windows.contains(&cache.active_hwnd) {
                                cache.active_hwnd
                            } else {
                                current_id
                            };
                            state_windows = merge_window_order(&cache.windows, &current_windows);
                            index = next_window_index(cache.index, state_windows.len(), reverse)
                                .unwrap_or(0);
                        }
                    }
                }
                if state_windows.is_empty() {
                    self.switch_windows_state.cache = None;
                    return Ok(false);
                }
                index = index.min(state_windows.len() - 1);
                let target_hwnd = match state_windows.get(index).copied() {
                    Some(window) if is_window_valid(window) => window,
                    _ => {
                        self.switch_windows_state.cache = None;
                        return Ok(false);
                    }
                };
                if !set_foreground_window(target_hwnd) {
                    self.switch_windows_state.cache = None;
                    return Ok(false);
                }

                self.switch_windows_state.cache = Some(SwitchWindowsCache {
                    module_path,
                    active_hwnd: state_id,
                    index,
                    windows: state_windows,
                });
                self.switch_windows_state.modifier_released = false;
                Ok(true)
            }
        }
    }

    fn switch_cached_window(&mut self, reverse: bool) -> bool {
        let Some(mut cache) = self.switch_windows_state.cache.take() else {
            return false;
        };
        cache.windows.retain(|window| is_window_valid(*window));
        if cache.windows.len() < 2 {
            return false;
        }

        let Some(index) = next_window_index(cache.index, cache.windows.len(), reverse) else {
            return false;
        };
        let target_hwnd = cache.windows[index];
        if !set_foreground_window(target_hwnd) {
            return false;
        }

        cache.index = index;
        self.switch_windows_state.cache = Some(cache);
        true
    }

    fn switch_apps(&mut self, reverse: bool) -> Result<()> {
        let _timer = StageTimer::new("switch_apps");
        self.apply_icon_results();
        self.icon_cache
            .cleanup_for_state(self.switch_apps_state.as_ref());
        self.icon_cache
            .retry_visible(self.switch_apps_state.as_ref());
        debug!(
            "switch apps: reverse={reverse} active={}",
            self.switch_apps_state
                .as_ref()
                .map(|state| state.apps.len())
                .unwrap_or(0)
        );
        if let Some(mut state) = self.switch_apps_state.take() {
            state
                .apps
                .retain(|entry| is_window_valid(entry.representative_hwnd));
            let active_keys: HashSet<String> = state
                .apps
                .iter()
                .map(|entry| entry.module_path.clone())
                .collect();
            self.icon_cache.cleanup(&active_keys);
            if state.apps.is_empty() {
                self.painter.unpaint(state);
                self.icon_cache.trim(None);
                return Ok(());
            }

            state.index = state.index.min(state.apps.len() - 1);
            if reverse {
                if state.index == 0 {
                    state.index = state.apps.len() - 1;
                } else {
                    state.index -= 1;
                }
            } else if state.index == state.apps.len() - 1 {
                state.index = 0;
            } else {
                state.index += 1;
            };
            debug!("switch apps: new index:{}", state.index);
            self.switch_apps_state = Some(state);
            return Ok(());
        }
        let windows = list_windows_with_cache(
            self.config.switch_apps_ignore_minimal,
            self.config.switch_apps_only_current_desktop(),
            self.is_admin,
            &mut self.process_metadata,
        )?;
        let active_keys: HashSet<String> = windows.keys().cloned().collect();
        self.icon_cache.cleanup(&active_keys);
        let mut apps = vec![];
        for (module_path, hwnds) in windows.iter() {
            if apps.len() >= MAX_SWITCH_APPS {
                warn!("switch app limit reached; showing first {MAX_SWITCH_APPS} applications");
                break;
            }
            let valid_hwnds: Vec<HWND> = hwnds
                .iter()
                .map(|(window, _)| *window)
                .filter(|window| is_window_valid(*window))
                .collect();
            if valid_hwnds.is_empty() {
                continue;
            }
            let module_hwnd = if is_iconic_window(valid_hwnds[0]) {
                valid_hwnds.last().copied().unwrap_or(valid_hwnds[0])
            } else {
                valid_hwnds[0]
            };
            let module_hicon = self.icon_cache.icon_for_app(module_path, module_hwnd);
            apps.push(AppEntry {
                module_path: module_path.clone(),
                icon: module_hicon,
                representative_hwnd: module_hwnd,
                window_count: valid_hwnds.len(),
            });
        }
        if apps.is_empty() {
            self.icon_cache.trim(None);
            return Ok(());
        }

        let index = if apps.len() == 1 {
            0
        } else if reverse {
            apps.len() - 1
        } else {
            1
        };

        let state = SwitchAppsState { apps, index };
        self.switch_apps_state = Some(state);
        self.icon_cache.trim(self.switch_apps_state.as_ref());
        debug!(
            "switch apps state ready apps={} index={}",
            self.switch_apps_state
                .as_ref()
                .map(|state| state.apps.len())
                .unwrap_or(0),
            index
        );
        Ok(())
    }

    fn apply_icon_results(&mut self) {
        let repaint = self
            .icon_cache
            .apply_results(self.switch_apps_state.as_mut());
        self.icon_cache.trim(self.switch_apps_state.as_ref());

        if repaint {
            if let Some(state) = self.switch_apps_state.as_ref() {
                self.painter.paint(state);
            }
        }
    }

    fn click(&mut self) {
        if let Some(state) = self.switch_apps_state.as_mut() {
            if let Some(i) = self.painter.find_clicked_app_index(state) {
                state.index = i;
                self.do_switch_app();
            }
        }
    }

    fn do_switch_app(&mut self) {
        if let Some(state) = self.switch_apps_state.take() {
            if let Some(entry) = state.apps.get(state.index) {
                if !set_foreground_window(entry.representative_hwnd) {
                    warn!(
                        "switch app target is no longer valid: {:?}",
                        entry.representative_hwnd
                    );
                }
            }
            self.painter.unpaint(state);
            self.icon_cache.trim(None);
        }
    }

    fn cancel_switch_app(&mut self) {
        if let Some(state) = self.switch_apps_state.take() {
            self.painter.unpaint(state);
            self.icon_cache.trim(None);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.cancel_tray_retry();
    }
}

fn install_app(hwnd: HWND, app: Box<App>) -> Result<()> {
    let app_ptr = Box::into_raw(app);
    if let Err(err) = check_error(|| set_window_user_data(hwnd, app_ptr as _)) {
        unsafe {
            drop(Box::from_raw(app_ptr));
        }
        return Err(anyhow!("Failed to set window ptr, {err}"));
    }
    Ok(())
}

fn take_app(hwnd: HWND) -> Result<Option<Box<App>>> {
    let ptr = check_error(|| set_window_user_data(hwnd, 0 as _))
        .map_err(|err| anyhow!("Failed to clear window ptr, {err}"))? as isize;
    if ptr == 0 {
        return Ok(None);
    }
    Ok(Some(unsafe { Box::from_raw(ptr as *mut App) }))
}

fn with_app<T>(hwnd: HWND, callback: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    let ptr = check_error(|| get_window_user_data(hwnd))
        .map_err(|err| anyhow!("Failed to get window ptr, {err}"))? as isize;
    if ptr == 0 {
        return Err(anyhow!("Window app pointer is null"));
    }

    let app = unsafe { &mut *(ptr as *mut App) };
    if app.hwnd != hwnd {
        return Err(anyhow!("Window app pointer belongs to another window"));
    }
    callback(app)
}

#[derive(Debug)]
struct SwitchWindowsState {
    cache: Option<SwitchWindowsCache>,
    modifier_released: bool,
}

#[derive(Debug)]
struct SwitchWindowsCache {
    module_path: String,
    active_hwnd: HWND,
    index: usize,
    windows: Vec<HWND>,
}

fn merge_window_order(cached: &[HWND], current: &[HWND]) -> Vec<HWND> {
    let mut remaining = current.to_vec();
    let mut ordered = Vec::with_capacity(current.len());
    for window in cached {
        if !is_window_valid(*window) {
            continue;
        }
        if let Some(index) = remaining.iter().position(|candidate| candidate == window) {
            ordered.push(*window);
            remaining.swap_remove(index);
        }
    }
    ordered.extend(remaining);
    ordered
}

fn next_window_index(index: usize, len: usize, reverse: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let index = index.min(len - 1);
    Some(if reverse {
        if index == 0 {
            len - 1
        } else {
            index - 1
        }
    } else if index == len - 1 {
        0
    } else {
        index + 1
    })
}

#[derive(Debug)]
pub struct AppEntry {
    pub module_path: String,
    pub icon: HICON,
    pub representative_hwnd: HWND,
    pub window_count: usize,
}

#[derive(Debug)]
pub struct SwitchAppsState {
    pub apps: Vec<AppEntry>,
    pub index: usize,
}

#[cfg(test)]
mod tests {
    use super::next_window_index;

    #[test]
    fn next_window_index_handles_empty_lists() {
        assert_eq!(next_window_index(0, 0, false), None);
        assert_eq!(next_window_index(3, 0, true), None);
    }

    #[test]
    fn next_window_index_clamps_stale_indices() {
        assert_eq!(next_window_index(9, 3, false), Some(0));
        assert_eq!(next_window_index(9, 3, true), Some(1));
    }

    #[test]
    fn next_window_index_wraps_in_both_directions() {
        assert_eq!(next_window_index(0, 3, true), Some(2));
        assert_eq!(next_window_index(2, 3, false), Some(0));
    }
}
