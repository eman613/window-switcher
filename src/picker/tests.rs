use super::*;
use crate::{
    config::Language, icon_cache::IconKey, layout::PixelRect,
    utils::window_identity::WindowIdentity,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};

mod dpi;
mod interaction_regressions;
mod lifecycle;

fn row(index: usize, title: &str) -> PickerRow {
    PickerRow {
        key: IconKey {
            group: Arc::from("fixture.exe"),
            identity: WindowIdentity::fixture(index),
        },
        primary: "Fixture".into(),
        secondary: title.into(),
        meta: "Administrator".into(),
        icon: None,
        remembered: Default::default(),
    }
}

#[test]
fn search_retains_native_strings_and_stable_layout_while_details_keeps_caption() {
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let text = Text::new(Language::English);
    let mut search =
        PickerWindow::create(HWND::default(), target.clone(), text, ViewKind::Search).unwrap();
    let area = PixelRect {
        left: -10000,
        top: -10000,
        right: -9200,
        bottom: -9200,
    };
    let monitor = MonitorSnapshot {
        identity: 1,
        screen: area,
        available: area,
        dpi: 96,
    };
    search.position(&Config::default(), monitor).unwrap();
    search
        .replace_rows(vec![row(1, "First\nwindow"), row(2, "Second 窗口")], 1, 7)
        .unwrap();
    assert_eq!(search.selected(), Some(1));
    let list = search.controls().list;
    let mut name = [0u16; 256];
    let length = unsafe {
        SendMessageW(
            list,
            LB_GETTEXT,
            Some(WPARAM(1)),
            Some(LPARAM(name.as_mut_ptr() as isize)),
        )
    }
    .0;
    assert!(length > 0);
    let name = String::from_utf16(&name[..length as usize]).unwrap();
    assert!(
        name.contains("Fixture") && name.contains("Second 窗口") && name.contains("Administrator")
    );
    search.state().visible.set(true);
    search.layout().unwrap();
    search.state().flags.set(0);
    search.layout().unwrap();
    assert_eq!(
        search.state().flags.get(),
        0,
        "identical region must not request another layout"
    );
    search.failure().unwrap();
    assert!(search.state().busy.get() && search.state().failed.get());
    search.replace_rows(Vec::new(), 0, 8).unwrap();
    assert!(!search.state().busy.get() && !search.state().failed.get());
    assert_eq!(search.selected(), None);
    assert_eq!(
        unsafe { GetWindowLongW(search.hwnd, GWL_STYLE) } as u32 & WS_CAPTION.0,
        0
    );
    let details = PickerWindow::create(HWND::default(), target, text, ViewKind::Details).unwrap();
    assert_eq!(
        unsafe { GetWindowLongW(details.hwnd, GWL_STYLE) } as u32 & WS_CAPTION.0,
        WS_CAPTION.0
    );
    search.hide();
    assert!(search.state().visual.borrow().rows.is_empty());
}

#[test]
fn unchanged_results_keep_the_scrolled_viewport_away_from_selection() {
    let mut search = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let area = PixelRect {
        left: -10000,
        top: -10000,
        right: -9200,
        bottom: -9200,
    };
    search
        .position(
            &Config::default(),
            MonitorSnapshot {
                identity: 1,
                screen: area,
                available: area,
                dpi: 96,
            },
        )
        .unwrap();
    let rows = || {
        (0..40)
            .map(|index| row(index, &format!("Window {index}")))
            .collect()
    };
    search.replace_rows(rows(), 0, 1).unwrap();
    let list = search.controls().list;
    unsafe {
        SendMessageW(list, LB_SETTOPINDEX, Some(WPARAM(30)), None);
    }
    let top = unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0;
    assert!(top >= 30);
    search.replace_rows(rows(), 0, 2).unwrap();
    assert_eq!(
        unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
        top
    );
    assert_eq!(search.selected(), Some(0));
}

#[test]
fn minimal_search_chrome_and_content_height_preserve_the_configured_row_limit() {
    let mut search = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let area = PixelRect {
        left: -10000,
        top: -10000,
        right: -9200,
        bottom: -9200,
    };
    search
        .position(
            &Config::default(),
            MonitorSnapshot {
                identity: 1,
                screen: area,
                available: area,
                dpi: 96,
            },
        )
        .unwrap();
    assert!(search.state().clear.get().is_invalid());
    assert_eq!(
        unsafe { GetWindowLongW(search.controls().status, GWL_STYLE) } as u32 & WS_VISIBLE.0,
        0
    );
    let heading = unsafe { GetDlgItem(Some(search.hwnd), 103) }.unwrap();
    assert_eq!(
        unsafe { GetWindowLongW(heading, GWL_STYLE) } as u32 & WS_VISIBLE.0,
        0
    );
    fn height(hwnd: HWND) -> i32 {
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
        rect.bottom - rect.top
    }
    search.fit_results(2).unwrap();
    let short = height(search.hwnd);
    search.fit_results(6).unwrap();
    let tall = height(search.hwnd);
    assert!(tall > short);
    search.fit_results(100).unwrap();
    let capped = height(search.hwnd);
    assert!(capped > tall);
    search.fit_results(1000).unwrap();
    assert_eq!(height(search.hwnd), capped);
    search.state().help_open.set(true);
    search.layout().unwrap();
    search.fit_results(1000).unwrap();
    assert_eq!(
        height(search.hwnd),
        capped,
        "help must not resize or recenter the search surface"
    );
}

#[test]
#[ignore = "requires an isolated interactive desktop and moves the real mouse"]
fn custom_scrollbar_drag_survives_refresh_and_reaches_last_row() {
    use std::time::{Duration, Instant};
    use windows::Win32::{
        Foundation::POINT,
        UI::Input::KeyboardAndMouse::{
            GetCapture, GetFocus, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
        },
    };
    fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: u32) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: flags,
                    mouseData: data,
                    ..Default::default()
                },
            },
        };
        assert_eq!(
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) },
            1
        );
    }
    fn mouse(down: bool) {
        mouse_input(
            if down {
                MOUSEEVENTF_LEFTDOWN
            } else {
                MOUSEEVENTF_LEFTUP
            },
            0,
        );
    }
    fn pump() {
        let until = Instant::now() + Duration::from_millis(150);
        while Instant::now() < until {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    struct MouseRestore(POINT);
    impl Drop for MouseRestore {
        fn drop(&mut self) {
            mouse(false);
            let _ = unsafe { SetCursorPos(self.0.x, self.0.y) };
        }
    }
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.unwrap();
    let _mouse_restore = MouseRestore(cursor);
    let mut search = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let area = PixelRect {
        left: 100,
        top: 100,
        right: 900,
        bottom: 800,
    };
    search
        .show(
            &Config::default(),
            MonitorSnapshot {
                identity: 1,
                screen: area,
                available: area,
                dpi: 96,
            },
        )
        .unwrap();
    let rows = || {
        (0..40)
            .map(|index| row(index, &format!("Window {index}")))
            .collect()
    };
    search.replace_rows(rows(), 0, 1).unwrap();
    pump();
    let mut window = windows::Win32::Foundation::RECT::default();
    unsafe { GetWindowRect(search.hwnd, &mut window) }.unwrap();
    let (row_height, text_height) = search.controls().metrics(search.state(), 96);
    let layout = layout::PickerLayout::calculate(
        window.right - window.left,
        window.bottom - window.top,
        96,
        ViewKind::Search,
        row_height,
        text_height,
        false,
    )
    .unwrap();
    let x = window.right - 12;
    unsafe { SetCursorPos(x, window.top + layout.list.top + 10) }.unwrap();
    mouse(true);
    pump();
    assert_eq!(unsafe { GetCapture() }, search.hwnd);
    unsafe { SetCursorPos(x, window.top + layout.list.bottom + 30) }.unwrap();
    pump();
    let list = search.controls().list;
    let top = unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0;
    assert!(top >= 30, "drag did not reach bottom: {top}");
    search.replace_rows(rows(), 0, 2).unwrap();
    pump();
    assert_eq!(unsafe { GetCapture() }, search.hwnd);
    assert_eq!(
        unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
        top
    );
    mouse(false);
    pump();
    assert_ne!(unsafe { GetCapture() }, search.hwnd);
    assert_eq!(
        unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
        top
    );
    let mut lines = 0u32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&mut lines as *mut u32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .unwrap();
    assert_ne!(
        lines, 0,
        "interactive wheel verification needs scrolling enabled"
    );
    for (name, x, y) in [
        (
            "list",
            window.left + layout.list.left + 50,
            window.top + layout.list.top + 20,
        ),
        (
            "edit",
            window.left + layout.edit.left + 40,
            window.top + layout.edit.top + 5,
        ),
        (
            "track",
            window.right - 12,
            window.top + layout.list.top + 20,
        ),
        (
            "help",
            window.left + layout.dismiss.left + 5,
            window.top + layout.dismiss.top + 5,
        ),
    ] {
        unsafe { SetCursorPos(x, y) }.unwrap();
        mouse_input(MOUSEEVENTF_WHEEL, 120);
        pump();
        let up = unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0;
        assert!(up < top, "real wheel over {name} did not scroll up: {up}");
        mouse_input(MOUSEEVENTF_WHEEL, (-120i32) as u32);
        pump();
        assert_eq!(
            unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
            top,
            "{name}"
        );
        assert_eq!(search.selected(), Some(0));
        assert_eq!(unsafe { GetFocus() }, search.controls().edit);
        search.replace_rows(rows(), 0, 3).unwrap();
        pump();
        assert_eq!(
            unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
            top
        );
    }
    for _ in 0..10 {
        pump();
    }
    assert!(
        !scroll_visibility::visible(search.state()),
        "idle indicator did not auto-hide"
    );
    search
        .state()
        .scroll_mode
        .set(crate::config::ScrollBarMode::Hidden);
    mouse_input(MOUSEEVENTF_WHEEL, 120);
    pump();
    assert!(unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0 < top);
    assert!(!scroll_visibility::visible(search.state()));
    unsafe {
        SetCursorPos(
            window.left + layout.dismiss.left + 7,
            window.top + layout.dismiss.top + 7,
        )
    }
    .unwrap();
    mouse(true);
    mouse(false);
    pump();
    assert!(search.state().help_open.get());
    search.layout().unwrap();
    mouse(true);
    mouse(false);
    pump();
    assert!(
        !search.state().help_open.get(),
        "the second rapid help click was lost"
    );
    search.hide();
}
