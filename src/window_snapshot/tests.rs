use super::*;
use crate::utils;
use windows::{
    core::{w, PCWSTR},
    Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SetWindowLongPtrW, SetWindowTextW, GWLP_HWNDPARENT,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    },
};

pub(super) struct Fixture(pub(super) HWND);
impl Fixture {
    pub(super) fn new(tool: bool) -> Self {
        let style = if tool {
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
        } else {
            WS_EX_NOACTIVATE
        };
        Self(
            unsafe {
                CreateWindowExW(
                    style,
                    w!("STATIC"),
                    w!("snapshot fixture"),
                    WS_POPUP | WS_VISIBLE,
                    -10000,
                    -10000,
                    800,
                    600,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        )
    }
    fn title(&self, text: &str) {
        let text = utils::to_wstring(text);
        unsafe { SetWindowTextW(self.0, PCWSTR(text.as_ptr())) }.unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) }.unwrap();
    }
}

#[test]
fn native_title_buffer_boundaries_preserve_full_unicode_text() {
    let window = Fixture::new(false);
    for title in [
        String::new(),
        "短标题".into(),
        "x".repeat(511),
        "x".repeat(512),
        "x".repeat(1023),
        format!("{}😀", "x".repeat(510)),
        "窗口".repeat(1000),
        "x".repeat(32766),
    ] {
        window.title(&title);
        assert_eq!(utils::get_window_title(window.0), title);
    }
}

#[test]
fn long_native_titles_and_process_metadata_survive_background_scan() {
    let window = Fixture::new(false);
    let title = "窗口标题 ".repeat(900);
    window.title(&title);
    assert_eq!(utils::get_window_title(window.0), title);
    let groups = scan_for_tools(false, false, true).unwrap();
    assert!(groups
        .values()
        .flatten()
        .any(|(hwnd, value)| *hwnd == window.0 && value == &title));
    let config = Config::default();
    let registry = lifetimes::WindowLifetimes::default();
    let mut metadata = ProcessMetadataCache::new(&config);
    let mut scan = scan::Scan::begin(
        filter::WindowFilter::from_config(&config, SwitchKind::Apps),
        true,
        0,
        0,
    )
    .unwrap();
    assert!(!scan.step(&mut metadata, &registry, Duration::ZERO, |_| true));
    assert!(scan.groups_is_empty());
}

#[test]
fn native_tool_topmost_untitled_and_title_exclusions_follow_ini_filters() {
    let window = Fixture::new(true);
    let mut config = Config::default();
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_none());
    config.switch_apps_include_topmost = true;
    config.switch_apps_include_tool_windows = true;
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_some());
    window.title("");
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_none());
    config.switch_apps_include_untitled = true;
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_some());
    window.title("Windows Input Experience");
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_none());
    config.switch_apps_exclude_titles.clear();
    assert!(filter::WindowFilter::from_config(&config, SwitchKind::Apps)
        .allows(window.0)
        .is_some());
    let registry = lifetimes::WindowLifetimes::default();
    let mut metadata = ProcessMetadataCache::new(&config);
    let mut scan = scan::Scan::begin(
        filter::WindowFilter::from_config(&config, SwitchKind::Apps),
        true,
        0,
        window.0 .0 as usize,
    )
    .unwrap();
    while !scan.step(&mut metadata, &registry, Duration::from_millis(50), |_| {
        false
    }) {}
    assert!(!scan
        .finish()
        .unwrap()
        .groups
        .values()
        .flatten()
        .any(|record| record.identity.hwnd() == window.0));
}

#[test]
fn deferred_process_exclusion_rejects_other_own_top_level_and_owned_windows() {
    let excluded = Fixture::new(false);
    let sibling = Fixture::new(false);
    let owned = Fixture::new(false);
    unsafe { SetWindowLongPtrW(owned.0, GWLP_HWNDPARENT, sibling.0 .0 as isize) };
    assert_eq!(utils::get_owner_window(owned.0), sibling.0);
    let config = Config::default();
    let filter = filter::WindowFilter::from_config(&config, SwitchKind::Apps);
    assert!(filter.allows(sibling.0).is_some());
    assert!(filter.allows(owned.0).is_some());
    let registry = lifetimes::WindowLifetimes::default();
    let mut metadata = ProcessMetadataCache::new(&config);
    let mut scan = scan::Scan::begin(filter, true, 0, excluded.0 .0 as usize).unwrap();
    while !scan.step(&mut metadata, &registry, Duration::from_millis(50), |_| {
        false
    }) {}
    let snapshot = scan.finish().unwrap();
    assert!(!snapshot
        .groups
        .values()
        .flatten()
        .any(|record| { [excluded.0, sibling.0, owned.0].contains(&record.identity.hwnd()) }));
}
