use super::*;
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::{POINT, RECT},
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEINPUT,
        },
        WindowsAndMessaging::*,
    },
};

#[test]
#[ignore = "requires an interactive desktop; temporarily focuses an owned search window and moves the mouse"]
fn real_mouse_confirms_a_displayed_result_while_background_refresh_is_pending() {
    struct Restore(POINT, HWND);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                let _ = SetCursorPos(self.0.x, self.0.y);
                let _ = SetForegroundWindow(self.1);
            }
        }
    }
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.unwrap();
    let _restore = Restore(cursor, unsafe { GetForegroundWindow() });
    let (mut session, _) = pending_session();
    let config = Config::default();
    let monitor = MonitorSnapshot::capture(&config, _restore.1).unwrap();
    session.window.show(&config, monitor).unwrap();
    session
        .window
        .fixture_rows(
            session
                .results
                .iter()
                .map(|entry| entry.row(session.text))
                .collect(),
            7,
        )
        .unwrap();
    session.window.fit_results(1).unwrap();
    let list = unsafe { GetDlgItem(Some(session.hwnd()), 102) }.unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let pump = || unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    };
    pump();
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(list, &mut bounds) }.unwrap();
    let point = POINT {
        x: bounds.left + 24,
        y: bounds.top + 12,
    };
    assert!(point.x < bounds.right && point.y < bounds.bottom);
    assert_eq!(unsafe { GetForegroundWindow() }, session.hwnd());
    assert_eq!(
        unsafe { WindowFromPoint(point) },
        list,
        "fixture row is occluded"
    );
    unsafe { SetCursorPos(point.x, point.y) }.unwrap();
    for flags in [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        assert_eq!(
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) },
            1
        );
        std::thread::sleep(Duration::from_millis(30));
        pump();
    }
    loop {
        pump();
        match session.poll().unwrap() {
            Some(SearchAction::Activate(entry)) => {
                assert_eq!(entry.identity, WindowIdentity::fixture(1));
                break;
            }
            Some(SearchAction::Cancel) => panic!("fixture lost focus"),
            None => assert!(Instant::now() < deadline, "mouse confirmation was lost"),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
