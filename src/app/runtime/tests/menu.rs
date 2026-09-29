use super::*;
use std::cell::Cell;
use windows::Win32::UI::WindowsAndMessaging::{
    EndMenu, GetGUIThreadInfo, GUITHREADINFO, GUI_INMENUMODE, WM_RBUTTONUP,
};

const PROBE_TIMER: usize = 0xa10f;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Observation {
    menus: u32,
    borrowed: u32,
    deferred: bool,
    errors: u32,
}

thread_local! {
    static OBSERVATION: Cell<Observation> = Cell::new(Observation::default());
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn tick(hwnd: HWND, _: u32, _: usize, _: u32) {
    if !ACTIVE.get() {
        return;
    }
    let mut observation = OBSERVATION.get();
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let in_menu = unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut gui) }.is_ok()
        && gui.flags.0 & GUI_INMENUMODE.0 != 0
        && gui.hwndMenuOwner == hwnd;
    if in_menu {
        observation.menus += 1;
    } else {
        observation.errors += 1;
    }
    let pointer = get_window_user_data(hwnd);
    if pointer != 0 {
        // Registration and its borrowed owner outlive this timer on this thread.
        let owner = unsafe { &*(pointer as *const AppHost) };
        if owner.app.try_borrow_mut().is_err() {
            observation.borrowed += 1;
        }
        if observation.menus == 1 && in_menu {
            let before = owner.pending.borrow().len();
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_USER_TRAYICON,
                    None,
                    Some(LPARAM(WM_RBUTTONUP as _)),
                );
            }
            observation.deferred = owner.pending.borrow().len() == before + 1;
        }
    } else {
        observation.errors += 1;
    }
    if unsafe { EndMenu() }.is_err() {
        observation.errors += 1;
    }
    OBSERVATION.set(observation);
}

struct MenuTimer(HWND);
impl Drop for MenuTimer {
    fn drop(&mut self) {
        // KillTimer does not remove callbacks already queued as WM_TIMER.
        ACTIVE.set(false);
        let _ = unsafe { KillTimer(Some(self.0), PROBE_TIMER) };
    }
}

struct RestoreForeground(HWND);
impl Drop for RestoreForeground {
    fn drop(&mut self) {
        if unsafe { IsWindow(Some(self.0)) }.as_bool() {
            crate::utils::set_foreground_window(self.0, || true);
        }
    }
}

#[test]
#[ignore = "requires an interactive desktop and Explorer; run as an isolated native regression"]
fn modal_menu_defers_a_second_tray_request_until_the_first_menu_returns() {
    let _foreground = RestoreForeground(crate::utils::get_foreground_window());
    let fixture = native_fixture();
    let registration = AppRegistration::new(fixture.window.0, &fixture.owner).unwrap();
    {
        let mut app = fixture.owner.app.borrow_mut();
        app.trayicon = Some(TrayIcon::create().unwrap());
        app.trayicon
            .as_mut()
            .unwrap()
            .register(fixture.window.0)
            .unwrap();
        app.startup.state = crate::startup::StartupState::Ready(false);
    }
    assert!(crate::utils::set_foreground_window(
        fixture.window.0,
        || true
    ));
    OBSERVATION.set(Observation::default());
    assert_ne!(
        unsafe { SetTimer(Some(fixture.window.0), PROBE_TIMER, 100, Some(tick)) },
        0
    );
    let timer = MenuTimer(fixture.window.0);
    ACTIVE.set(true);
    unsafe {
        SendMessageW(
            fixture.window.0,
            WM_USER_TRAYICON,
            None,
            Some(LPARAM(WM_RBUTTONUP as _)),
        );
    }
    drop(timer);
    let observed = OBSERVATION.get();
    unsafe { tick(HWND::default(), WM_TIMER, PROBE_TIMER, 0) };
    assert_eq!(OBSERVATION.get(), observed);
    assert_eq!(observed.errors, 0);
    assert_eq!(observed.menus, 2);
    assert_eq!(observed.borrowed, 2);
    assert!(observed.deferred);
    assert!(fixture.owner.pending.borrow().is_empty());
    assert!(fixture.owner.target.is_live());
    drop(registration);
    assert_eq!(get_window_user_data(fixture.window.0), 0);
}
