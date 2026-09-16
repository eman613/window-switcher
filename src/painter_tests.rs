use super::*;
use crate::{
    app::AppEntry,
    badge::BadgeStyle,
    config::Config,
    icon_cache::{IconCache, IconKey},
    icon_loader::native,
    layout::{MonitorSnapshot, PixelRect},
    utils::window_identity::WindowIdentity,
    window_snapshot::lifetimes::WindowLifetimes,
};
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
            CreateWindowExW, DestroyWindow, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

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

fn state(hwnd: HWND) -> SwitchAppsState {
    let native = native::fallback().unwrap();
    let icon = IconCache::fixture(native::rasterize(native.0, 256).unwrap());
    let identity = WindowIdentity::capture(hwnd, &WindowLifetimes::default()).unwrap();
    let screen = PixelRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    SwitchAppsState {
        apps: (0..5)
            .map(|i| AppEntry {
                application: crate::app_identity::AppIdentity::plain(format!("fixture-{i}").into()),
                windows: Arc::from([]),
                key: IconKey {
                    group: format!("fixture-{i}").into(),
                    identity,
                },
                icon: Some(icon.clone()),
                window_count: if i == 0 { 100 } else { i + 1 },
                executable: "fixture.exe".into(),
                display_name: format!("Application {i}").into(),
            })
            .collect(),
        index: 0,
        show_badge: true,
        badge_max: 99,
        badge_style: BadgeStyle::from_config(&Config::default()),
        monitor: MonitorSnapshot {
            identity: 1,
            screen,
            available: screen,
            dpi: 96,
        },
        revision: 0,
    }
}

#[test]
fn hidden_panel_preserves_alpha_and_recovers_from_native_submit_failure() {
    let window = TestWindow::new();
    let mut painter = GdiAAPainter::new(window.0, &Config::default()).unwrap();
    let mut state = state(window.0);
    for rounded in [false, true] {
        painter.invalidate();
        painter.appearance.rounded = rounded;
        painter.render(&state).unwrap();
        let pixels = painter.scene.as_ref().unwrap().surface.pixels().unwrap();
        assert_eq!(pixels[3], if rounded { 0 } else { 255 });
        for pixel in pixels.as_chunks::<4>().0.iter() {
            assert!(pixel[..3].iter().all(|c| *c <= pixel[3]));
        }
        painter.hwnd = HWND::default();
        assert!(painter.render(&state).is_err());
        painter.hwnd = window.0;
        painter.render(&state).unwrap();
    }
    state.apps[0].icon = None;
    painter.render(&state).unwrap();
    state.apps.clear();
    assert!(painter.render(&state).is_err());
    assert!(!painter.show);
}

#[test]
fn selection_reuses_rasters_and_last_page_hit_testing_matches_drawn_geometry() {
    let window = TestWindow::new();
    let config = Config {
        max_columns: 3,
        ..Default::default()
    };
    let mut painter = GdiAAPainter::new(window.0, &config).unwrap();
    let mut state = state(window.0);
    painter.render(&state).unwrap();
    let pixels = painter.scene.as_ref().unwrap().sprites[0]
        .plain
        .data
        .as_ptr();
    state.index = 1;
    painter.render(&state).unwrap();
    assert_eq!(
        pixels,
        painter.scene.as_ref().unwrap().sprites[0]
            .plain
            .data
            .as_ptr()
    );
    let first_bounds = painter.layout().unwrap().bounds;
    state.index = 4;
    painter.render(&state).unwrap();
    let last = painter.layout().unwrap();
    assert_eq!(first_bounds, last.bounds);
    assert_eq!(last.page, 1);
    let item = &last.items[1];
    assert_eq!(
        last.hit_test(
            last.bounds.left + item.outer.left,
            last.bounds.top + item.outer.top
        ),
        Some(4)
    );
}

#[test]
fn cancellation_during_render_does_not_show_or_focus_the_panel() {
    let window = TestWindow::new();
    let mut painter = GdiAAPainter::new(window.0, &Config::default()).unwrap();
    let checks = std::cell::Cell::new(0);
    painter
        .paint(&state(window.0), || {
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

#[test]
fn background_alpha_does_not_dim_sprites_or_bottom_name_and_pointer_states_are_distinct() {
    let window = TestWindow::new();
    let mut config = Config {
        app_name_mode: crate::config::AppNameMode::Selected,
        app_name_text_color: Some(0xffffff),
        background_color: Some(0x112233),
        panel_corner_radius: Some(0),
        selection_color: Some(0x445566),
        icon_background_opacity: 0,
        ..Default::default()
    };
    let state = state(window.0);
    let mut baseline = None;
    for opacity in [0, 1, 50, 100] {
        config.background_opacity = opacity;
        let mut painter = GdiAAPainter::new(window.0, &config).unwrap();
        painter.fonts = Some(
            crate::font_resources::FontResources::load(&config, &std::env::temp_dir()).unwrap(),
        );
        painter.render(&state).unwrap();
        let scene = painter.scene.as_ref().unwrap();
        let sprite = scene.sprites[1].plain.data.clone();
        if let Some(baseline) = &baseline {
            assert_eq!(&sprite, baseline);
        } else {
            baseline = Some(sprite);
        }
        let footer = scene.layout.footer.unwrap();
        let pixels = scene.surface.pixels().unwrap();
        assert_eq!(pixels[3], ((opacity * 255 + 50) / 100) as u8);
        let width = scene.layout.bounds.width();
        assert!(
            (footer.top..footer.bottom).any(|y| (footer.left..footer.right).any(|x| {
                let pixel = &pixels[((y * width + x) * 4) as usize..][..4];
                pixel[0] >= 200 && pixel[1] >= 200 && pixel[2] >= 200 && pixel[3] >= 200
            })),
            "name vanished at {opacity}%"
        );
        let idle = pixels.to_vec();
        painter.hover(Some(state.apps[1].key.clone()));
        painter.render(&state).unwrap();
        let hovered = painter
            .scene
            .as_ref()
            .unwrap()
            .surface
            .pixels()
            .unwrap()
            .to_vec();
        assert_ne!(idle, hovered);
        painter.press(Some(state.apps[1].key.clone()));
        painter.render(&state).unwrap();
        assert_ne!(
            hovered,
            painter.scene.as_ref().unwrap().surface.pixels().unwrap()
        );
        painter.release();
        painter.render(&state).unwrap();
        assert_eq!(
            hovered,
            painter.scene.as_ref().unwrap().surface.pixels().unwrap()
        );
    }
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
fn stage_c_resource_stress() {
    let window = TestWindow::new();
    let mut painter = GdiAAPainter::new(window.0, &Config::default()).unwrap();
    let mut state = state(window.0);
    for iteration in 0..200 {
        state.index = iteration % state.apps.len();
        painter.render(&state).unwrap();
    }
    let baseline = resources();
    eprintln!(
        "stage=c iterations=0 gdi={} user={} handles={}",
        baseline.0, baseline.1, baseline.2
    );
    let mut samples = Vec::with_capacity(10_000);
    for iteration in 1..=10_000 {
        state.index = iteration % state.apps.len();
        if iteration % 1000 == 0 {
            state.apps[0].window_count = 100 + iteration;
            painter.invalidate();
            painter.appearance.rounded = iteration % 2000 == 0;
        }
        if iteration % 2000 == 0 {
            state.monitor.dpi = [96, 144, 192][iteration / 2000 % 3];
        }
        let started = Instant::now();
        painter.render(&state).unwrap();
        samples.push(started.elapsed().as_micros());
        if iteration % 1000 == 0 {
            let sample = resources();
            eprintln!(
                "stage=c iterations={iteration} gdi={} user={} handles={}",
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
        "stage=c hidden_render samples={} p50_us={} p95_us={} p99_us={}",
        samples.len(),
        samples[4999],
        samples[9499],
        samples[9899]
    );
}
