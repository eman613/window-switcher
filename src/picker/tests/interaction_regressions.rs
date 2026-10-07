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
fn leaving_confirmation_invalidates_the_complete_old_and_new_rows() {
    use windows::Win32::Graphics::Gdi::{GetUpdateRect, ValidateRect};
    let search = fixture();
    unsafe {
        let _ = ShowWindow(search.hwnd, SW_SHOWNOACTIVATE);
    }
    let state = search.state();
    state.close.configure(&Config {
        close_enable: true,
        ..Default::default()
    });
    let targets = state
        .visual
        .borrow()
        .rows
        .iter()
        .map(|row| (row.key.identity, row.primary.clone()))
        .collect();
    search.update_close_targets(targets, || Ok(())).unwrap();
    super::super::close_confirmation::command(state, super::super::close_confirmation::CLOSE);
    assert!(super::super::close_confirmation::confirming(state));
    let list = state.list.get();
    let mut first = windows::Win32::Foundation::RECT::default();
    let mut second = windows::Win32::Foundation::RECT::default();
    unsafe {
        SendMessageW(
            list,
            LB_GETITEMRECT,
            Some(WPARAM(0)),
            Some(LPARAM(&mut first as *mut _ as isize)),
        );
        SendMessageW(
            list,
            LB_GETITEMRECT,
            Some(WPARAM(1)),
            Some(LPARAM(&mut second as *mut _ as isize)),
        );
        SendMessageW(list, LB_SETCURSEL, Some(WPARAM(1)), None);
        let _ = ValidateRect(Some(list), None);
    }
    state.signal(0);
    assert!(!super::super::close_confirmation::confirming(state));
    let mut dirty = windows::Win32::Foundation::RECT::default();
    assert!(unsafe { GetUpdateRect(list, Some(&mut dirty), false) }.as_bool());
    assert!(dirty.left <= first.left && dirty.right >= first.right);
    assert!(dirty.top <= first.top && dirty.bottom >= second.bottom);
}

#[test]
fn action_layout_shares_the_row_end_without_a_scrollbar() {
    let search = fixture();
    search.replace_rows(vec![row(0, "Window")], 0, 2).unwrap();
    let bounds = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 720,
        bottom: 31,
    };
    for dpi in [96, 144, 192] {
        search.state().dpi.set(dpi);
        let actions = super::super::row_actions::layout(search.state(), bounds, true);
        assert_eq!(actions.right, bounds.right);
        assert_eq!(actions.left + 2 * actions.width, bounds.right);
    }
}

#[test]
fn row_repaint_removes_previous_selection_edges() {
    use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED, ODT_LISTBOX};

    let search = fixture();
    for radius in [0, 8, 16] {
        search
            .state()
            .visual
            .borrow_mut()
            .skin
            .as_mut()
            .unwrap()
            .selection_radius = radius;
        let background = search
            .state()
            .visual
            .borrow()
            .skin
            .as_ref()
            .unwrap()
            .palette
            .surface;
        let mut actual = crate::render_surface::RenderSurface::new(320, 48).unwrap();
        let mut expected = crate::render_surface::RenderSurface::new(320, 48).unwrap();
        actual.fill_rgb(background).unwrap();
        expected.fill_rgb(background).unwrap();
        let mut item = DRAWITEMSTRUCT {
            CtlType: ODT_LISTBOX,
            CtlID: messages::LIST_ID as u32,
            itemID: 0,
            hDC: actual.dc(),
            rcItem: windows::Win32::Foundation::RECT {
                left: 2,
                top: 2,
                right: 318,
                bottom: 46,
            },
            itemState: windows::Win32::UI::Controls::ODS_FLAGS(ODS_SELECTED.0 | ODS_FOCUS.0),
            ..Default::default()
        };
        assert!(paint::item(search.state(), &item).unwrap());
        item.itemState = Default::default();
        assert!(paint::item(search.state(), &item).unwrap());
        item.hDC = expected.dc();
        assert!(paint::item(search.state(), &item).unwrap());
        let mismatches = actual
            .pixels()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.pixels().unwrap().as_chunks::<4>().0)
            .filter(|(actual, expected)| actual[..3] != expected[..3])
            .count();
        assert_eq!(
            mismatches, 0,
            "radius={radius}: stale RGB pixels after deselection"
        );
    }
}

#[test]
fn hiding_scroll_overlay_restores_the_complete_selected_row() {
    use crate::config::ScrollBarMode;
    use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED, ODT_LISTBOX};

    let search = fixture();
    let state = search.state();
    let mut bounds = windows::Win32::Foundation::RECT::default();
    assert!(
        unsafe {
            SendMessageW(
                state.list.get(),
                LB_GETITEMRECT,
                Some(WPARAM(0)),
                Some(LPARAM(
                    (&mut bounds as *mut windows::Win32::Foundation::RECT) as isize,
                )),
            )
            .0
        } >= 0
    );
    let mut surface =
        crate::render_surface::RenderSurface::new(bounds.right, bounds.bottom).unwrap();
    surface
        .fill_rgb(state.visual.borrow().skin.as_ref().unwrap().palette.surface)
        .unwrap();
    let item = DRAWITEMSTRUCT {
        CtlType: ODT_LISTBOX,
        CtlID: messages::LIST_ID as u32,
        itemID: 0,
        hDC: surface.dc(),
        rcItem: bounds,
        itemState: ODS_SELECTED,
        ..Default::default()
    };
    state.scroll_mode.set(ScrollBarMode::Hidden);
    assert!(paint::item(state, &item).unwrap());
    let clean = surface.pixels().unwrap().to_vec();
    state.scroll_mode.set(ScrollBarMode::Always);
    assert!(paint::item(state, &item).unwrap());
    let changed: Vec<_> = surface
        .pixels()
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(clean.as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (actual, expected))| actual[..3] != expected[..3])
        .map(|(index, _)| index as i32 % bounds.right)
        .collect();
    assert!(!changed.is_empty(), "overlay must actually be drawn");
    assert!(
        changed
            .iter()
            .all(|x| *x >= bounds.right - 14 && *x < bounds.right - 2),
        "overlay must stay inside the row and clear of its border"
    );
    state.scroll_mode.set(ScrollBarMode::Hidden);
    assert!(paint::item(state, &item).unwrap());
    assert!(surface
        .pixels()
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(clean.as_chunks::<4>().0)
        .all(|(actual, expected)| actual[..3] == expected[..3]));
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

#[test]
fn identical_refresh_and_stationary_mouse_do_not_repaint_or_reveal_again() {
    use windows::Win32::Graphics::Gdi::{GetUpdateRect, ValidateRect};
    let search = fixture();
    let state = search.state();
    let list = search.controls().list;
    let point = LPARAM((12 << 16) | 30);
    unsafe {
        let _ = ShowWindow(search.hwnd, SW_SHOWNOACTIVATE);
        SendMessageW(list, WM_MOUSEMOVE, None, Some(point));
    }
    scroll_visibility::hide(search.hwnd, state);
    unsafe {
        let _ = ValidateRect(Some(list), None);
    }
    for epoch in 2..8 {
        search
            .replace_rows(
                (0..40).map(|index| row(index, "Window")).collect(),
                0,
                epoch,
            )
            .unwrap();
        unsafe {
            SendMessageW(list, WM_MOUSEMOVE, None, Some(point));
        }
        assert!(
            !state.scroll_hint.get(),
            "stationary messages must not reveal a hidden indicator"
        );
        assert!(
            !unsafe { GetUpdateRect(list, None, false) }.as_bool(),
            "unchanged rows must not invalidate the list"
        );
    }
}

#[test]
fn hover_invalidates_only_the_previous_and_current_rows() {
    use windows::Win32::Graphics::Gdi::{GetUpdateRect, ValidateRect};
    let search = fixture();
    let state = search.state();
    let list = search.controls().list;
    let height = unsafe { SendMessageW(list, LB_GETITEMHEIGHT, Some(WPARAM(0)), None) }.0 as i32;
    unsafe {
        let _ = ShowWindow(search.hwnd, SW_SHOWNOACTIVATE);
    }
    super::super::repaint::hover(state, Some(0));
    unsafe {
        let _ = ValidateRect(Some(list), None);
    }
    super::super::repaint::hover(state, Some(1));
    let mut update = windows::Win32::Foundation::RECT::default();
    assert!(unsafe { GetUpdateRect(list, Some(&mut update), false) }.as_bool());
    assert_eq!((update.top, update.bottom), (0, height * 2));
}

#[test]
fn held_identity_survives_same_query_rows_but_not_removal_or_query_change() {
    let search = fixture();
    let state = search.state();
    let identity = state.visual.borrow().rows[0].key.identity;
    state.pressed.set(Some(identity));
    search
        .replace_rows(
            (0..40).rev().map(|index| row(index, "Window")).collect(),
            0,
            2,
        )
        .unwrap();
    assert_eq!(state.pressed.get(), Some(identity));
    search
        .replace_rows((1..40).map(|index| row(index, "Window")).collect(), 0, 3)
        .unwrap();
    assert_eq!(state.pressed.get(), None);
    state.pressed.set(Some(identity));
    state.changed();
    assert_eq!(state.pressed.get(), None);
}

#[test]
fn changing_only_highlights_repaints_without_losing_the_scrolled_anchor() {
    use windows::Win32::Graphics::Gdi::{GetUpdateRect, ValidateRect};
    let search = fixture();
    let list = search.controls().list;
    unsafe {
        let _ = ShowWindow(search.hwnd, SW_SHOWNOACTIVATE);
        SendMessageW(list, LB_SETTOPINDEX, Some(WPARAM(12)), None);
        let _ = ValidateRect(Some(list), None);
    }
    let mut rows: Vec<_> = (0..40).map(|index| row(index, "Window")).collect();
    rows[12].highlights = RowHighlights {
        query: Arc::from("win"),
        primary: vec![],
        secondary: std::iter::once(0..3).collect(),
    };
    search.replace_rows(rows, 0, 2).unwrap();
    assert_eq!(
        unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }.0,
        12
    );
    assert!(unsafe { GetUpdateRect(list, None, false) }.as_bool());
    search.highlight_query("new query");
    assert_eq!(search.state().visual.borrow().query, "new query");
    assert_ne!(
        search.state().visual.borrow().rows[12]
            .highlights
            .query
            .as_ref(),
        "new query"
    );
}
