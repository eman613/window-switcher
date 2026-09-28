use super::*;
use crate::{
    config::Language, icon_cache::IconKey, layout::PixelRect,
    utils::window_identity::WindowIdentity,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};

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
