use super::*;
use crate::utils;
use windows::{
    core::{w, PCWSTR},
    Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SetWindowTextW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    },
};

struct Fixture(HWND);
impl Fixture {
    fn new(tool: bool) -> Self {
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
