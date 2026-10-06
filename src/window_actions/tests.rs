use super::*;
use windows::{core::w, Win32::UI::WindowsAndMessaging::*};

#[test]
fn self_and_stale_targets_are_rejected_without_closing_a_window() {
    let lifetimes = WindowLifetimes::default();
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("STATIC"),
            w!("close rejection fixture"),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let identity = WindowIdentity::capture(hwnd, &lifetimes).unwrap();
    assert_eq!(request_close(identity, &lifetimes), Err("self-window"));
    let mut stale = identity;
    stale.process.pid ^= 1;
    assert_eq!(request_close(stale, &lifetimes), Err("stale-window"));
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
    unsafe { DestroyWindow(hwnd) }.unwrap();
    assert_eq!(request_close(stale, &lifetimes), Err("stale-window"));
}
