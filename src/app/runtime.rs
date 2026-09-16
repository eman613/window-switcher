use std::{cell::RefCell, collections::VecDeque, sync::Arc};

use anyhow::{bail, Context, Result};
use once_cell::sync::OnceCell;
use windows::{
    core::w,
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::WM_MOUSELEAVE,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                GetWindowLongPtrW, IsWindow, KillTimer, LoadCursorW, PostQuitMessage,
                RegisterClassW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW,
                TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWL_STYLE, HTCLIENT,
                IDC_ARROW, MSG, WINDOW_STYLE, WM_CANCELMODE, WM_CAPTURECHANGED, WM_COMMAND,
                WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_GETOBJECT,
                WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY,
                WM_NCHITTEST, WM_SETFOCUS, WM_SETTINGCHANGE, WM_THEMECHANGED, WM_TIMER, WNDCLASSW,
                WS_CAPTION, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            },
        },
    },
};

use super::{
    App, SwitchWindowsState, NAME, WM_SCENE_INVALIDATED, WM_USER_CONFIG_CHANGED,
    WM_USER_REGISTER_TRAYICON, WM_USER_TRAYICON,
};
use crate::{
    accessibility::{Accessibility, WM_ACCESSIBILITY},
    config::LoadedConfig,
    font_resources::WM_FONTS,
    foreground::ForegroundWatcher,
    icon_loader::{IconService, WM_ICON},
    keyboard::{
        dispatch::{InputDispatch, WM_INPUT_READY},
        KeyboardListener,
    },
    painter::GdiAAPainter,
    restart::{ChildSession, WM_RESTART},
    startup::{Startup, WM_STARTUP},
    trayicon::TrayIcon,
    utils::{
        check_error, com::ComApartment, get_window_user_data, is_running_as_admin,
        set_window_user_data, SingleInstance,
    },
    window_snapshot::{SnapshotService, WM_SNAPSHOT},
    window_target::WindowTarget,
};

const INPUT_POLL_TIMER: usize = 0xa101;
const MAX_DEFERRED_MESSAGES: usize = 64;
static WINDOW_CLASS: OnceCell<u16> = OnceCell::new();

pub(super) fn run(
    loaded: &LoadedConfig,
    instance: SingleInstance,
    child: Option<ChildSession>,
    diagnostics: crate::diagnostics::Diagnostics,
) -> Result<()> {
    let _com = ComApartment::sta()?;
    let window = ApplicationWindow::create()?;
    let hwnd = window.0;
    let mut painter = GdiAAPainter::new(hwnd, &loaded.config)?;
    let lifetimes = Arc::new(crate::window_snapshot::lifetimes::WindowLifetimes::default());
    let foreground = ForegroundWatcher::init(&loaded.config, hwnd, lifetimes.clone())?;
    let is_admin = is_running_as_admin()?;
    let startup = Startup::default();
    let taskbar_message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_message == 0 {
        return Err(windows::core::Error::from_win32()).context("ui stage=taskbar-message");
    }
    let target = Arc::new(WindowTarget::new(hwnd));
    let text = crate::localization::Text::new(loaded.config.language);
    let accessibility = Accessibility::new(target.clone(), text).context("uia stage=initialize")?;
    let accessible_root = accessibility.root.clone();
    let search = loaded
        .config
        .search_enable
        .then(|| crate::search::SearchSession::new(hwnd, &loaded.config, target.clone(), text))
        .transpose()?;
    let details = (loaded.config.switch_apps_enable && loaded.config.details_enable)
        .then(|| crate::window_details::WindowDetails::new(hwnd, target.clone(), text))
        .transpose()?;
    let preview = loaded
        .config
        .preview_enable
        .then(|| crate::preview::WindowPreview::new(hwnd, target.clone(), text));
    if loaded.config.switch_apps_enable {
        painter.start_fonts(
            loaded
                .path
                .parent()
                .context("font stage=config-directory")?,
            target.clone(),
        );
    }
    let snapshots = SnapshotService::start(
        &loaded.config,
        loaded
            .path
            .parent()
            .context("snapshot stage=config-directory")?,
        is_admin,
        foreground.status(),
        lifetimes.clone(),
        target.clone(),
    )?;
    let icons = IconService::start(
        &loaded.config,
        loaded
            .path
            .parent()
            .context("icon stage=config-directory")?,
        lifetimes,
        target.clone(),
    )?;
    let input = Arc::new(InputDispatch::with_metrics(
        target.clone(),
        loaded.config.metrics_enabled,
    ));
    input.set_paused(loaded.config.input_paused);
    info!("pause stage=initial paused={}", loaded.config.input_paused);
    let lifecycle = super::lifecycle::Lifecycle::new(instance, child, loaded);
    lifecycle.attach(target.clone());
    let replacement = lifecycle.is_replacement();
    let owner = Box::new(AppHost {
        app: RefCell::new(App {
            hwnd,
            is_admin,
            painter,
            accessibility,
            startup,
            pause: crate::pause::PauseControl::new(loaded.path.clone(), target.clone()),
            config: loaded.config.clone(),
            trayicon: loaded.config.trayicon.then(TrayIcon::create).transpose()?,
            config_watcher: None,
            switch_windows_state: SwitchWindowsState {
                modifier_released: true,
                ..Default::default()
            },
            switch_apps_state: None,
            search,
            details,
            preview,
            snapshots,
            icons,
            remembered_icons: Default::default(),
            switching: Default::default(),
            target: target.clone(),
            input: input.clone(),
            input_session: 0,
            lifecycle,
            feedback: Default::default(),
            text,
            diagnostics,
        }),
        target: target.clone(),
        accessible_root,
        taskbar_message,
        pending: RefCell::new(VecDeque::with_capacity(MAX_DEFERRED_MESSAGES)),
    });
    // A single Box owns App until the callback registration has been cleared.
    // Callback code only borrows App through RefCell for one dispatch at a time.
    let registration = AppRegistration::new(hwnd, &owner)?;
    let timer = InputPollTimer::new(hwnd)?;
    let keyboard = KeyboardListener::with_activation(
        input.clone(),
        foreground.status(),
        &loaded.config.to_hotkeys(),
        !replacement,
        loaded.config.injected_events,
    )?;
    owner.app.borrow_mut().lifecycle.activation = Some(keyboard.activation());
    // Wake the owner as soon as hook readiness is confirmed; the timer remains
    // a fallback for a full message queue, not the normal startup trigger.
    target.try_post(WM_INPUT_READY);
    let result = eventloop();
    target.close();
    drop(keyboard);
    drop(timer);
    drop(registration);
    drop(owner);
    input.log_summary();
    // Painter/DC/App drop before HWND, apartment after all Shell consumers.
    drop(foreground);
    drop(window);
    result
}

#[derive(Clone, Copy)]
struct OwnedMessage {
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
}

struct AppHost {
    app: RefCell<App>,
    target: Arc<WindowTarget>,
    accessible_root: windows::Win32::UI::Accessibility::IRawElementProviderSimple,
    taskbar_message: u32,
    pending: RefCell<VecDeque<OwnedMessage>>,
}

impl AppHost {
    fn owns(&self, msg: u32, wparam: WPARAM) -> bool {
        matches!(
            msg,
            WM_USER_CONFIG_CHANGED
                | WM_USER_TRAYICON
                | WM_USER_REGISTER_TRAYICON
                | WM_INPUT_READY
                | WM_RESTART
                | WM_STARTUP
                | WM_SNAPSHOT
                | WM_ICON
                | WM_FONTS
                | WM_ACCESSIBILITY
                | WM_SCENE_INVALIDATED
                | WM_COMMAND
                | WM_LBUTTONUP
                | WM_LBUTTONDOWN
                | WM_MOUSEMOVE
                | WM_MOUSELEAVE
                | WM_CAPTURECHANGED
                | WM_CANCELMODE
                | WM_SETFOCUS
                | WM_KILLFOCUS
        ) || msg == self.taskbar_message
            || (msg == WM_TIMER && wparam.0 == INPUT_POLL_TIMER)
    }

    fn dispatch(&self, message: OwnedMessage) {
        let mut next = Some(message);
        while self.target.is_live() {
            let Some(message) = next else {
                break;
            };
            if self.apply(message) {
                next = self.pending.borrow_mut().pop_front();
                continue;
            }
            // Only owned scalar messages are deferred. Never retain OS pointers
            // from messages such as WM_SETTINGCHANGE after their callback returns.
            let mut pending = self.pending.borrow_mut();
            if matches!(
                message.msg,
                WM_INPUT_READY
                    | WM_TIMER
                    | WM_USER_REGISTER_TRAYICON
                    | WM_SNAPSHOT
                    | WM_ICON
                    | WM_FONTS
                    | WM_ACCESSIBILITY
                    | WM_SCENE_INVALIDATED
                    | WM_MOUSEMOVE
            ) && pending.iter().any(|old| old.msg == message.msg)
            {
                return;
            }
            if pending.len() == MAX_DEFERRED_MESSAGES {
                self.target.close();
                error!("ui stage=deferred-queue overflow; closing safely");
                unsafe { PostQuitMessage(1) };
                return;
            }
            pending.push_back(message);
            return;
        }
    }

    fn apply(&self, message: OwnedMessage) -> bool {
        let Ok(mut app) = self.app.try_borrow_mut() else {
            return false;
        };
        let result = if matches!(
            message.msg,
            WM_INPUT_READY
                | WM_TIMER
                | WM_RESTART
                | WM_STARTUP
                | WM_SNAPSHOT
                | WM_ICON
                | WM_FONTS
                | WM_ACCESSIBILITY
        ) {
            app.drain_input();
            Ok(())
        } else if message.msg == WM_USER_REGISTER_TRAYICON || message.msg == self.taskbar_message {
            if message.msg == self.taskbar_message {
                app.feedback.retry_count = 0;
            }
            app.set_trayicon();
            Ok(())
        } else {
            app.handle_message(message.msg, message.wparam, message.lparam)
        };
        if let Err(err) = result {
            app.report_failure(
                crate::localization::FailureKind::Interface,
                &format!("{err:#}"),
            );
        }
        true
    }
}

struct AppRegistration<'a> {
    hwnd: HWND,
    owner: &'a AppHost,
}

impl<'a> AppRegistration<'a> {
    fn new(hwnd: HWND, owner: &'a AppHost) -> Result<Self> {
        check_error(|| set_window_user_data(hwnd, owner as *const AppHost as _))
            .context("ui stage=register-owner")?;
        Ok(Self { hwnd, owner })
    }
}

impl Drop for AppRegistration<'_> {
    fn drop(&mut self) {
        self.owner.target.close();
        set_window_user_data(self.hwnd, 0);
    }
}

struct InputPollTimer(HWND);
impl InputPollTimer {
    fn new(hwnd: HWND) -> Result<Self> {
        // Safety fallback for queue saturation / failed terminal notifications.
        if unsafe { SetTimer(Some(hwnd), INPUT_POLL_TIMER, 50, None) } == 0 {
            bail!("input stage=terminal-timer failed");
        }
        Ok(Self(hwnd))
    }
}
impl Drop for InputPollTimer {
    fn drop(&mut self) {
        let _ = unsafe { KillTimer(Some(self.0), INPUT_POLL_TIMER) };
    }
}

struct ApplicationWindow(HWND);
impl ApplicationWindow {
    fn create() -> Result<Self> {
        let module = unsafe { GetModuleHandleW(None) }.context("ui stage=module")?;
        WINDOW_CLASS.get_or_try_init(|| -> Result<u16> {
            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }?;
            let class = WNDCLASSW {
                hCursor: cursor,
                hInstance: HINSTANCE(module.0),
                lpszClassName: NAME,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&class) };
            if atom == 0 {
                return Err(windows::core::Error::from_win32()).context("ui stage=register-class");
            }
            Ok(atom)
        })?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                NAME,
                NAME,
                WINDOW_STYLE(0),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(module.into()),
                None,
            )
        }
        .context("ui stage=create-window")?;
        let window = Self(hwnd);
        let style = check_error(|| unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) })? as u32;
        check_error(|| unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !WS_CAPTION.0) as isize)
        })?;
        Ok(window)
    }
}
impl Drop for ApplicationWindow {
    fn drop(&mut self) {
        if unsafe { IsWindow(Some(self.0)) }.as_bool() {
            if let Err(err) = unsafe { DestroyWindow(self.0) } {
                warn!("ui stage=destroy-window code={:#x}", err.code().0);
            }
        }
    }
}

fn eventloop() -> Result<()> {
    let mut message = MSG::default();
    loop {
        match unsafe { GetMessageW(&mut message, None, 0, 0) }.0 {
            -1 => return Err(windows::core::Error::from_win32()).context("ui stage=message-loop"),
            0 => {
                return if message.wParam.0 == 0 {
                    Ok(())
                } else {
                    anyhow::bail!("ui stage=message-loop interrupted");
                }
            }
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTCLIENT as isize);
    }
    if msg == WM_ERASEBKGND {
        return LRESULT(0);
    }
    let pointer = get_window_user_data(hwnd);
    if pointer != 0 {
        let host = &*(pointer as *const AppHost);
        if msg == WM_GETOBJECT
            && lparam.0 as i32 == windows::Win32::UI::Accessibility::UiaRootObjectId
            && host.target.is_live()
        {
            // UIA must receive its LRESULT immediately, even during a reentrant
            // native callback. The provider owns values, never a borrow of App.
            return windows::Win32::UI::Accessibility::UiaReturnRawElementProvider(
                hwnd,
                wparam,
                lparam,
                &host.accessible_root,
            );
        }
        if matches!(msg, WM_DESTROY | WM_NCDESTROY) {
            host.target.close();
        }
        if matches!(
            msg,
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE | WM_THEMECHANGED
        ) {
            // Never retain pointer-bearing native message parameters across a callback.
            host.target.try_post(WM_SCENE_INVALIDATED);
        }
        if host.owns(msg, wparam) {
            if matches!(
                msg,
                WM_INPUT_READY
                    | WM_USER_CONFIG_CHANGED
                    | WM_USER_REGISTER_TRAYICON
                    | WM_RESTART
                    | WM_STARTUP
                    | WM_SNAPSHOT
                    | WM_ICON
                    | WM_FONTS
                    | WM_ACCESSIBILITY
                    | WM_SCENE_INVALIDATED
            ) && !host.target.accepts(lparam)
            {
                return LRESULT(0);
            }
            host.dispatch(OwnedMessage {
                msg,
                wparam,
                lparam,
            });
            return LRESULT(0);
        }
    }
    if msg == WM_NCDESTROY {
        set_window_user_data(hwnd, 0);
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    #[test]
    fn nested_native_messages_defer_mutation_and_destroy_retires_owner() {
        let window = ApplicationWindow::create().unwrap();
        let target = Arc::new(WindowTarget::new(window.0));
        let input = Arc::new(InputDispatch::new(target.clone()));
        let config = crate::config::Config::default();
        let lifetimes = Arc::new(crate::window_snapshot::lifetimes::WindowLifetimes::default());
        let foreground = ForegroundWatcher::init(&config, window.0, lifetimes.clone()).unwrap();
        let snapshots = SnapshotService::start(
            &config,
            &std::env::temp_dir(),
            false,
            foreground.status(),
            lifetimes.clone(),
            target.clone(),
        )
        .unwrap();
        let icons =
            IconService::start(&config, &std::env::temp_dir(), lifetimes, target.clone()).unwrap();
        let accessibility = Accessibility::new(
            target.clone(),
            crate::localization::Text::new(config.language),
        )
        .unwrap();
        let accessible_root = accessibility.root.clone();
        let owner = Box::new(AppHost {
            app: RefCell::new(App {
                hwnd: window.0,
                is_admin: false,
                trayicon: None,
                startup: Startup::default(),
                config: Default::default(),
                config_watcher: None,
                switch_windows_state: Default::default(),
                switch_apps_state: None,
                search: None,
                details: None,
                preview: None,
                pause: crate::pause::PauseControl::new(Default::default(), target.clone()),
                snapshots,
                icons,
                remembered_icons: Default::default(),
                switching: Default::default(),
                painter: GdiAAPainter::new(window.0, &config).unwrap(),
                accessibility,
                target: target.clone(),
                input,
                input_session: 0,
                lifecycle: Default::default(),
                feedback: Default::default(),
                text: crate::localization::Text::new(crate::config::Language::Chinese),
                diagnostics: Default::default(),
            }),
            target: target.clone(),
            accessible_root,
            taskbar_message: 0xffff,
            pending: RefCell::new(VecDeque::new()),
        });
        let registration = AppRegistration::new(window.0, &owner).unwrap();
        let held = owner.app.borrow_mut();
        unsafe {
            SendMessageW(window.0, WM_LBUTTONUP, None, None);
            SendMessageW(window.0, WM_LBUTTONUP, None, None);
            SendMessageW(window.0, WM_INPUT_READY, None, Some(LPARAM(-1)));
        }
        assert_eq!(owner.pending.borrow().len(), 2);
        drop(held);
        unsafe {
            SendMessageW(window.0, WM_LBUTTONUP, None, None);
        }
        assert!(owner.pending.borrow().is_empty());
        drop(registration);
        assert_eq!(get_window_user_data(window.0), 0);
        assert!(!target.is_live());
        assert!(!target.try_post(WM_INPUT_READY));
        drop(owner);
        drop(window);
    }
}
