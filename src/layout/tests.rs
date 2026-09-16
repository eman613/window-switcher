use super::*;

fn monitor(dpi: u32) -> MonitorSnapshot {
    let scale = dpi as i32 / 48;
    let screen = PixelRect {
        left: -960 * scale,
        top: -100 * scale,
        right: 0,
        bottom: 440 * scale,
    };
    MonitorSnapshot {
        identity: 1,
        screen,
        available: PixelRect {
            bottom: screen.bottom - 20 * scale,
            ..screen
        },
        dpi,
    }
}

fn options() -> LayoutOptions {
    LayoutOptions::from_config(&Config::default())
}

#[test]
fn natural_single_axis_and_dual_axis_fit_obey_dip_contract() {
    for dpi in [96, 144, 192] {
        let scale = f64::from(dpi) / 96.0;
        for (width, height, expected_width, expected_height, icon) in [
            (0, 0, 380, 92, 64),
            (740, 0, 740, 164, 136),
            (740, 92, 740, 92, 64),
            (0, 164, 740, 164, 136),
        ] {
            let layout = LayoutSnapshot::calculate(
                &LayoutOptions {
                    panel_width: width,
                    panel_height: height,
                    ..options()
                },
                monitor(dpi),
                5,
                1,
                0,
            )
            .unwrap();
            assert_eq!(
                layout.bounds.width(),
                (f64::from(expected_width) * scale) as i32
            );
            assert_eq!(
                layout.bounds.height(),
                (f64::from(expected_height) * scale) as i32
            );
            assert_eq!(layout.icon_size, (f64::from(icon) * scale) as i32);
        }
    }
}

#[test]
fn pagination_preserves_last_page_geometry_and_screen_hit_rectangles() {
    for dpi in [96, 144, 192] {
        for count in [1, 5, 50, 250, 4096] {
            let first = LayoutSnapshot::calculate(&options(), monitor(dpi), count, 0, 0).unwrap();
            let last =
                LayoutSnapshot::calculate(&options(), monitor(dpi), count, count - 1, 0).unwrap();
            assert_eq!(first.bounds, last.bounds);
            assert_eq!(first.icon_size, last.icon_size);
            assert_eq!(first.capacity, last.capacity);
            assert!(last.items.len() <= last.capacity);
            assert!(last.bounds.top >= last.monitor.available.top);
            assert!(last.bounds.bottom <= last.monitor.available.bottom);
            for item in &last.items {
                assert_eq!(
                    last.hit_test(
                        last.bounds.left + item.outer.left,
                        last.bounds.top + item.outer.top
                    ),
                    Some(item.index)
                );
                assert!(item.icon.left >= item.outer.left && item.icon.right <= item.outer.right);
            }
            assert_eq!(last.hit_test(last.bounds.right, last.bounds.bottom), None);
        }
    }
}

#[test]
fn small_targets_page_instead_of_squeezing_icons_and_limits_do_not_rewrite_targets() {
    let mut settings = options();
    settings.panel_width = 740;
    settings.panel_height = 500;
    settings.max_width = 300;
    settings.max_height = 100;
    settings.max_columns = 4;
    let layout = LayoutSnapshot::calculate(&settings, monitor(96), 250, 249, 0).unwrap();
    assert_eq!(layout.bounds.width(), 300);
    assert_eq!(layout.bounds.height(), 100);
    assert_eq!(layout.capacity, 4);
    assert_eq!(layout.icon_size, 62);
    assert_eq!(settings.panel_width, 740);
    assert_eq!(settings.panel_height, 500);
    assert!(layout.footer.is_none());
}

#[test]
fn narrow_automatic_axes_shrink_to_the_minimum_before_rejecting() {
    for dpi in [96, 144, 192] {
        let mut narrow = monitor(dpi);
        narrow.available.right = narrow.available.left + (52 * dpi / 96) as i32;
        let layout = LayoutSnapshot::calculate(&options(), narrow, 50, 49, 0).unwrap();
        assert_eq!(layout.capacity, 1);
        assert_eq!(layout.icon_size, (24 * dpi / 96) as i32);
        assert_eq!(layout.bounds.width(), (52 * dpi / 96) as i32);
        narrow.available.right -= (dpi / 96) as i32;
        assert!(LayoutSnapshot::calculate(&options(), narrow, 1, 0, 0).is_err());
    }
}

#[test]
fn unusable_counts_work_areas_and_explicit_combinations_are_rejected() {
    for (count, selected) in [(0, 0), (4097, 0), (3, 3)] {
        assert!(LayoutSnapshot::calculate(&options(), monitor(96), count, selected, 0).is_err());
    }
    let mut settings = options();
    settings.panel_width = 51;
    assert!(settings.validate().is_err());
    settings.panel_width = 52;
    assert!(settings.validate().is_ok());
    let mut tiny = monitor(96);
    tiny.available.right = tiny.available.left + 20;
    assert!(LayoutSnapshot::calculate(&options(), tiny, 1, 0, 0).is_err());
    assert!(LayoutSnapshot::calculate(&options(), monitor(96), 1, 0, 2000).is_err());
}

#[test]
fn selected_name_reserves_fixed_readable_height_and_rejects_incompatible_targets() {
    let mut config = Config {
        app_name_mode: crate::config::AppNameMode::Selected,
        app_name_font_size: 48,
        ..Default::default()
    };
    let options = LayoutOptions::from_config(&config);
    assert_eq!(options.name_height, 78);
    let named =
        LayoutSnapshot::calculate(&options, monitor(96), 5, 0, options.name_height).unwrap();
    let plain = LayoutSnapshot::calculate(&options, monitor(96), 5, 0, 0).unwrap();
    assert_eq!(named.bounds.height() - plain.bounds.height(), 78);
    assert_eq!(named.icon_size, plain.icon_size);
    assert_eq!(named.footer.unwrap().height(), 78);
    config.panel_height = 129;
    assert!(LayoutOptions::from_config(&config).validate().is_err());
    config.panel_height = 130;
    assert!(LayoutOptions::from_config(&config).validate().is_ok());
    config.app_name_mode = crate::config::AppNameMode::Off;
    assert_eq!(LayoutOptions::from_config(&config).name_height, 0);
}
