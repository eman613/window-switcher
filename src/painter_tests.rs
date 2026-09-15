use std::time::Instant;
use windows::{
    core::w,
    Win32::{
        Foundation::HWND,
        Graphics::Gdi::GdiFlush,
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{
                GetCurrentProcess, GetGuiResources, GetProcessHandleCount, GR_GDIOBJECTS,
                GR_USEROBJECTS,
            },
        },
        UI::WindowsAndMessaging::{
            CopyIcon, CreateWindowExW, DestroyIcon, DestroyWindow, LoadIconW, HICON,
            IDI_APPLICATION, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

use super::*;
use crate::{app::AppEntry, badge::BadgeStyle, config::Config};

struct TestWindow(HWND);
impl TestWindow {
    fn new() -> Self {
        Self(
            unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    w!("isolated rendering fixture"),
                    WS_POPUP,
                    0,
                    0,
                    320,
                    200,
                    None,
                    None,
                    Some(GetModuleHandleW(None).unwrap().into()),
                    None,
                )
            }
            .unwrap(),
        )
    }
}
impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) }.unwrap();
    }
}

struct TestIcon(HICON);
impl TestIcon {
    fn new() -> Self {
        Self(unsafe { CopyIcon(LoadIconW(None, IDI_APPLICATION).unwrap()) }.unwrap())
    }
}
impl Drop for TestIcon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) }.unwrap();
    }
}

fn state(icon: HICON) -> SwitchAppsState {
    SwitchAppsState {
        apps: vec![AppEntry {
            icon,
            representative_hwnd: HWND::default(),
            window_count: 100,
            identity: None,
        }],
        index: 0,
        show_badge: true,
        badge_max: 99,
        badge_style: BadgeStyle::from_config(&Config::default()),
    }
}

#[test]
fn complete_hidden_panel_renders_and_recovers_from_failed_native_operations() {
    let window = TestWindow::new();
    let icon = TestIcon::new();
    let mut painter = GdiAAPainter::new(window.0).unwrap();
    for rounded in [false, true] {
        painter.rounded_corner = rounded;
        painter.render(&state(icon.0)).unwrap();
        assert!(painter.render(&state(HICON::default())).is_err());
        painter.render(&state(icon.0)).unwrap();
    }
    let mut empty = state(icon.0);
    empty.apps.clear();
    assert!(painter.render(&empty).is_err());
    assert!(
        !painter.show,
        "render-only validation must not show or focus a window"
    );
}

#[test]
fn cancellation_during_render_does_not_show_or_focus_the_panel() {
    let window = TestWindow::new();
    let icon = TestIcon::new();
    let mut painter = GdiAAPainter::new(window.0).unwrap();
    let checks = std::cell::Cell::new(0);
    painter
        .paint(&state(icon.0), || {
            checks.set(checks.get() + 1);
            checks.get() == 1
        })
        .unwrap();
    assert!(checks.get() >= 2);
    assert!(!painter.show);
    assert!(
        !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(window.0) }.as_bool()
    );
}

fn resources() -> (u32, u32, u32) {
    unsafe {
        let _ = GdiFlush();
        let mut handles = 0;
        GetProcessHandleCount(GetCurrentProcess(), &mut handles).unwrap();
        (
            GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS),
            GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS),
            handles,
        )
    }
}

#[test]
#[ignore = "isolated 10000-iteration Windows resource stress; run single-threaded"]
fn stage_a_resource_stress() {
    let window = TestWindow::new();
    let icon = TestIcon::new();
    let mut painter = GdiAAPainter::new(window.0).unwrap();
    let state = state(icon.0);
    for _ in 0..20 {
        painter.render(&state).unwrap();
    }
    assert!(crate::utils::get_module_path(std::process::id()).is_some());
    assert!(crate::utils::is_process_elevated(std::process::id()).is_some());
    let mutex_name = format!("WindowSwitcherStageAStress-{}", std::process::id());
    let instance = crate::utils::SingleInstance::create(&mutex_name).unwrap();
    assert!(instance.is_single());
    let baseline = resources();
    eprintln!(
        "stage=a iterations=0 gdi={} user={} handles={}",
        baseline.0, baseline.1, baseline.2
    );
    let mut samples = Vec::with_capacity(10_000);
    for iteration in 1..=10_000 {
        painter.rounded_corner = iteration % 2 == 0;
        let started = Instant::now();
        painter.render(&state).unwrap();
        samples.push(started.elapsed().as_micros());
        assert!(crate::utils::get_module_path(std::process::id()).is_some());
        assert!(crate::utils::is_process_elevated(std::process::id()).is_some());
        assert!(!crate::utils::SingleInstance::create(&mutex_name)
            .unwrap()
            .is_single());
        if iteration % 1000 == 0 {
            let sample = resources();
            eprintln!(
                "stage=a iterations={iteration} gdi={} user={} handles={}",
                sample.0, sample.1, sample.2
            );
            assert!(
                sample.0 <= baseline.0 + 2
                    && sample.1 <= baseline.1 + 2
                    && sample.2 <= baseline.2 + 4,
                "resource growth: {baseline:?} -> {sample:?}"
            );
        }
    }
    samples.sort_unstable();
    eprintln!(
        "stage=a hidden_render samples={} p50_us={} p95_us={} p99_us={}",
        samples.len(),
        samples[4999],
        samples[9499],
        samples[9899]
    );
    drop(instance);
    assert!(crate::utils::SingleInstance::create(&mutex_name)
        .unwrap()
        .is_single());
}
