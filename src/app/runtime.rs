use std::{cell::RefCell, collections::VecDeque, sync::Arc};

use anyhow::{bail, Context, Result};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Controls::WM_MOUSELEAVE,
            Input::Ime::ImmDisableIME,
            WindowsAndMessaging::{
                DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostQuitMessage,
                RegisterWindowMessageW, SetTimer, TranslateMessage, HTCLIENT, MSG, WM_CANCELMODE,
                WM_CAPTURECHANGED, WM_COMMAND, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
                WM_ERASEBKGND, WM_GETOBJECT, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MOUSEMOVE, WM_NCDESTROY, WM_NCHITTEST, WM_SETFOCUS, WM_SETTINGCHANGE,
                WM_THEMECHANGED, WM_TIMER,
            },
        },
    },
};

use super::{
    App, SwitchWindowsState, WM_SCENE_INVALIDATED, WM_USER_CONFIG_CHANGED,
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
mod window;
use window::ApplicationWindow;

pub(super) fn run(
    loaded: &LoadedConfig,
    instance: SingleInstance,
    child: Option<ChildSession>,
    diagnostics: crate::diagnostics::Diagnostics,
) -> Result<()> {
    let _com = ComApartment::sta()?;
    if !loaded.config.search_enable && !loaded.config.details_enable {
        // This process cannot create text entry surfaces until a config restart.
        let disabled = unsafe { ImmDisableIME(GetCurrentThreadId()) }.as_bool();
        debug!("ui stage=nontext-thread-ime disabled={disabled}");
    }
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
    let target = window.target();
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
            quick_settings: super::settings::QuickSettingsState::new(
                loaded.path.clone(),
                target.clone(),
            ),
            config: loaded.config.clone(),
            report: super::report::ReportState::new(target.clone()),
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
    let _callback_time = crate::diagnostics::ui::UiRegion::callback();
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
            let _uia_time = crate::diagnostics::ui::UiRegion::accessibility();
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
    let _default_time = crate::diagnostics::ui::UiRegion::default_proc(msg);
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests;
