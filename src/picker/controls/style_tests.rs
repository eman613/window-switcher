use super::*;
use crate::picker::PickerWindow;
use crate::{config::Language, window_target::WindowTarget};
use std::sync::Arc;
use windows::Win32::Graphics::Gdi::GetObjectType;

fn picker(kind: ViewKind) -> PickerWindow {
    PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        kind,
    )
    .unwrap()
}

#[test]
fn details_resource_failures_keep_previous_handles_and_allow_retry() {
    for stage in [1, 2] {
        let mut window = picker(ViewKind::Details);
        let state = window.state.as_deref().unwrap();
        let controls = window.controls.as_mut().unwrap();
        let config = Config::default();
        controls.style(window.hwnd, state, &config, 96).unwrap();
        let brush = state.brush.get();
        let font = controls.font.as_ref().unwrap().0;
        let colors = (state.background.get(), state.foreground.get());
        let height = controls.text_height.get();
        STYLE_FAILURE.with(|failure| failure.set(stage));
        let error = controls
            .style(window.hwnd, state, &config, 144)
            .unwrap_err();
        assert!(error.to_string().contains("injected-resource-failure"));
        assert_eq!(state.brush.get(), brush);
        assert_eq!(controls.font.as_ref().unwrap().0, font);
        assert_eq!((state.background.get(), state.foreground.get()), colors);
        assert_eq!(controls.text_height.get(), height);
        assert_eq!(state.dpi.get(), 96);
        assert_ne!(unsafe { GetObjectType(HGDIOBJ(brush.0)) }, 0);
        assert_ne!(unsafe { GetObjectType(font) }, 0);
        controls.style(window.hwnd, state, &config, 144).unwrap();
        assert_eq!(state.dpi.get(), 144);
        drop(window);
    }
}

#[test]
fn search_reentrant_style_failure_does_not_publish_unowned_handles() {
    let mut window = picker(ViewKind::Search);
    let state = window.state.as_deref().unwrap();
    let controls = window.controls.as_mut().unwrap();
    let config = Config::default();
    controls.style(window.hwnd, state, &config, 96).unwrap();
    let brush = state.brush.get();
    let font = unsafe { SendMessageW(controls.edit, WM_GETFONT, None, None) };
    let borrow = state.visual.borrow();
    assert!(controls.style(window.hwnd, state, &config, 144).is_err());
    assert_eq!(state.brush.get(), brush);
    assert_eq!(
        unsafe { SendMessageW(controls.edit, WM_GETFONT, None, None) },
        font
    );
    assert_eq!(state.dpi.get(), 96);
    assert_ne!(unsafe { GetObjectType(HGDIOBJ(brush.0)) }, 0);
    drop(borrow);
    controls.style(window.hwnd, state, &config, 144).unwrap();
    assert_eq!(state.dpi.get(), 144);
}
