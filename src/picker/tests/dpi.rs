use super::*;
use windows::Win32::Foundation::RECT;

#[test]
fn dpi_relayout_refreshes_monitor_before_fitting_results() {
    let config = Config::default();
    let mut window = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let original = MonitorSnapshot::for_window(&config, window.hwnd).unwrap();
    window.position(&config, original).unwrap();
    // Stale monitor identity proves relayout reads the actual destination.
    window.monitor.as_mut().unwrap().identity = 1;
    let dpi = if original.dpi == 144 { 192 } else { 144 };
    let mut rect = RECT::default();
    unsafe { GetWindowRect(window.hwnd, &mut rect) }.unwrap();
    unsafe {
        SendMessageW(
            window.hwnd,
            WM_DPICHANGED,
            Some(WPARAM((dpi | (dpi << 16)) as usize)),
            Some(LPARAM((&rect as *const RECT) as isize)),
        );
    }
    window.layout().unwrap();
    let refreshed = window.monitor.unwrap();
    assert_eq!(refreshed.identity, original.identity);
    assert_eq!(refreshed.dpi, dpi);
    window.fit_results(1).unwrap();
    assert_eq!(window.state().dpi.get(), dpi);
    assert_eq!(window.monitor.unwrap().dpi, dpi);
    assert_eq!(
        window.configuration.as_ref().unwrap().search_visible_rows,
        config.search_visible_rows
    );
}

#[test]
fn fitting_rows_preserves_a_valid_user_position_after_dragging() {
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
    let left = monitor.available.left + 10;
    let top = monitor.available.top + 10;
    unsafe {
        SetWindowPos(
            window.hwnd,
            None,
            left,
            top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
        SendMessageW(window.hwnd, WM_EXITSIZEMOVE, None, None);
    }
    window.layout().unwrap();
    window.fit_results(1).unwrap();
    let mut rect = RECT::default();
    unsafe { GetWindowRect(window.hwnd, &mut rect) }.unwrap();
    assert_eq!((rect.left, rect.top), (left, top));
}
