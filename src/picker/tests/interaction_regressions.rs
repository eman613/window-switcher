use super::*;
use windows::Win32::Graphics::Gdi::GetPixel;

fn fixture() -> PickerWindow {
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
            &Config {
                theme: crate::config::Theme::Dark,
                ..Default::default()
            },
            MonitorSnapshot {
                identity: 1,
                screen: area,
                available: area,
                dpi: 96,
            },
        )
        .unwrap();
    search
        .replace_rows((0..40).map(|index| row(index, "Window")).collect(), 0, 1)
        .unwrap();
    search.state().visible.set(true);
    search
}

#[test]
fn dark_help_resize_erases_with_the_current_theme_before_paint() {
    let mut search = fixture();
    let mut surface = crate::render_surface::RenderSurface::new(800, 800).unwrap();
    for _ in 0..4 {
        search.state().command(messages::DISMISS_ID);
        search.layout().unwrap();
        search.fit_results(40).unwrap();
        for control in [search.hwnd, search.state().dismiss.get()] {
            surface.fill_rgb(0xffffff).unwrap();
            let erased = unsafe {
                SendMessageW(
                    control,
                    WM_ERASEBKGND,
                    Some(WPARAM(surface.dc().0 as usize)),
                    None,
                )
            };
            assert_eq!(erased.0, 1);
            assert_eq!(
                unsafe { GetPixel(surface.dc(), 2, 2) },
                search.state().background.get(),
                "window and owner-drawn help button must erase with the current theme"
            );
        }
    }
}

#[test]
fn inline_help_keeps_fonts_and_viewport_until_style_is_invalidated() {
    let mut search = fixture();
    let list = search.controls().list;
    let font = unsafe { SendMessageW(list, WM_GETFONT, None, None) };
    for _ in 0..4 {
        search.state().command(messages::DISMISS_ID);
        search.layout().unwrap();
        search.fit_results(40).unwrap();
        assert_eq!(unsafe { SendMessageW(list, WM_GETFONT, None, None) }, font);
    }
    unsafe {
        SendMessageW(search.hwnd, WM_SETTINGCHANGE, None, None);
    }
    assert!(search.state().style_dirty.get());
    search.layout().unwrap();
    assert!(!search.state().style_dirty.get());
}

#[test]
fn wheel_over_search_controls_scrolls_without_changing_selection_or_query() {
    let search = fixture();
    let list = search.controls().list;
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
    for control in [
        list,
        search.controls().edit,
        search.state().dismiss.get(),
        search.hwnd,
    ] {
        unsafe {
            SendMessageW(list, LB_SETTOPINDEX, Some(WPARAM(0)), None);
            SendMessageW(
                control,
                WM_MOUSEWHEEL,
                Some(WPARAM(((-120i16 as u16) as usize) << 16)),
                None,
            );
        }
        let top = unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0;
        assert_eq!(
            top > 0,
            lines > 0,
            "wheel target {control:?}: top={top}, lines={lines}"
        );
        assert_eq!(search.selected(), Some(0));
        assert_eq!(search.query().unwrap(), "");
        unsafe {
            SendMessageW(control, WM_MOUSEWHEEL, Some(WPARAM(120 << 16)), None);
        }
        assert_eq!(
            unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
            0
        );
    }
}

#[test]
fn hidden_scrollbar_keeps_wheel_and_auto_indicator_has_a_bounded_lifetime() {
    let search = fixture();
    let state = search.state();
    state.scroll_mode.set(crate::config::ScrollBarMode::Auto);
    scroll_visibility::reveal(state);
    assert!(scroll_visibility::visible(state));
    scroll_visibility::hide(search.hwnd, state);
    assert!(!scroll_visibility::visible(state));
    state.scroll_mode.set(crate::config::ScrollBarMode::Always);
    assert!(scroll_visibility::visible(state));
    state.scroll_mode.set(crate::config::ScrollBarMode::Hidden);
    assert!(!scroll_visibility::visible(state));
    let list = search.controls().list;
    unsafe {
        SendMessageW(list, LB_SETTOPINDEX, Some(WPARAM(10)), None);
    }
    scrollbar::wheel(state, WPARAM(120 << 16));
    assert!(!scroll_visibility::visible(state));
    assert!(unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0 <= 10);
    assert_eq!(search.selected(), Some(0));
}
