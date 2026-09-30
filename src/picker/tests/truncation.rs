use super::*;
use windows::Win32::Foundation::RECT;

#[test]
fn truncated_results_show_an_inline_hint_without_resizing_the_window_or_list() {
    let config = Config::default();
    let mut window = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let monitor = MonitorSnapshot::for_window(&config, window.hwnd).unwrap();
    window.position(&config, monitor).unwrap();
    window
        .fixture_rows((0..50).map(|i| row(i, "Fixture window")).collect(), 7)
        .unwrap();
    let bounds = |hwnd| {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
        rect
    };
    let original_window = bounds(window.hwnd);
    let original_list = bounds(window.controls().list);
    let help = unsafe { GetDlgItem(Some(window.hwnd), messages::HELP_ID as i32) }.unwrap();
    for (shown, total, truncated) in [
        (0, 0, false),
        (1, 1, false),
        (50, 50, false),
        (50, 51, true),
        (50, 200, true),
        (1, 1, false),
    ] {
        window.result_count(shown, total).unwrap();
        assert_eq!(
            unsafe { GetWindowLongW(help, GWL_STYLE) } as u32 & WS_VISIBLE.0 != 0,
            truncated
        );
        assert_eq!(bounds(window.hwnd), original_window);
        assert_eq!(bounds(window.controls().list), original_list);
        assert_eq!(window.selected(), Some(0));
        if truncated {
            let mut text = [0u16; 128];
            let copied = unsafe { GetWindowTextW(help, &mut text) };
            assert_eq!(
                String::from_utf16_lossy(&text[..copied as usize]),
                "More matches: refine search"
            );
            let help_bounds = bounds(help);
            assert!(help_bounds.right > help_bounds.left);
            assert!(help_bounds.left >= bounds(window.controls().edit).right);
        }
    }
    window.result_count(50, 51).unwrap();
    window.state().command(messages::DISMISS_ID);
    window.layout().unwrap();
    assert!(window.state().help_open.get());
    assert!(window.state().truncated.get());
    window.state().command(messages::DISMISS_ID);
    window.layout().unwrap();
    window.pending().unwrap();
    assert!(!window.state().truncated.get());
    assert_eq!(
        unsafe { GetWindowLongW(help, GWL_STYLE) } as u32 & WS_VISIBLE.0,
        0
    );
}
