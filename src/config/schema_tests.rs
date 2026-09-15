use super::{document, schema::SETTINGS, Config, DEFAULT_CONFIG, SWITCH_APPS_HOTKEY_ID};

#[test]
fn every_declared_setting_has_template_default_nondefault_and_invalid_coverage() {
    let mut cases = vec![
        ("", "trayicon", "no", "maybe"),
        ("", "auto_restart", "no", "maybe"),
        ("", "restart_delay_ms", "200", "199"),
        ("switch-windows", "enable", "no", "maybe"),
        ("switch-windows", "hotkey", "ctrl+f11", "shift+alt+tab"),
        ("switch-windows", "blacklist", "Game.EXE", "bad\0.exe"),
        ("switch-windows", "ignore_minimal", "yes", "maybe"),
        ("switch-windows", "only_current_desktop", "no", "maybe"),
        ("switch-apps", "enable", "yes", "maybe"),
        ("switch-apps", "hotkey", "ctrl+f12", "shift+tab"),
        ("switch-apps", "ignore_minimal", "yes", "maybe"),
        (
            "switch-apps",
            "override_icons",
            "app.exe=custom.ico",
            "missing-equals",
        ),
        ("switch-apps", "show_badge", "no", "maybe"),
        ("switch-apps", "badge_max", "250", "1"),
        ("switch-apps", "badge_color", "#123456", "#XYZXYZ"),
        ("switch-apps", "badge_text_color", "#654321", "#FFFFFFFF"),
        ("switch-apps", "badge_font_size", "18", "25"),
        ("switch-apps", "only_current_desktop", "no", "maybe"),
        ("startup", "enabled", "yes", "maybe"),
        ("startup", "run_level", "standard", "admin"),
        ("startup", "battery_policy", "stop", "auto"),
        ("startup", "command_timeout_ms", "500", "499"),
        ("config", "watch_mode", "poll", "none"),
        ("config", "poll_interval_ms", "100", "99"),
        ("config", "restart_timeout_ms", "1000", "999"),
        ("config", "retry_delay_ms", "200", "199"),
        ("config", "retry_limit", "0", "11"),
        ("localization", "language", "en-US", "unknown"),
        ("log", "level", "debug", "verbose"),
        ("log", "path", "test.log", "bad\0.log"),
        ("log", "max_file_mb", "1", "101"),
        ("log", "retained_files", "0", "21"),
        ("performance", "metrics_enabled", "yes", "maybe"),
        ("performance", "metrics_interval_s", "1", "3601"),
        ("switch-apps", "blacklist", "Game.EXE", "bad\0.exe"),
        ("input", "unknown_foreground", "handle", "ignore"),
        ("input", "injected_events", "passthrough", "ignore"),
        ("appearance", "monitor", "primary", "last"),
        ("appearance", "use_work_area", "no", "maybe"),
        ("appearance", "panel_width", "740", "32769"),
        ("appearance", "panel_height", "164", "32769"),
        ("appearance", "icon_size", "128", "23"),
        ("appearance", "icon_padding", "8", "65"),
        ("appearance", "item_gap", "10", "65"),
        ("appearance", "panel_padding", "20", "129"),
        ("appearance", "max_width", "1024", "32769"),
        ("appearance", "max_height", "512", "32769"),
        ("appearance", "max_columns", "10", "129"),
        ("performance", "icon_cache_limit", "16", "15"),
        ("performance", "icon_cache_mb", "4", "257"),
        ("performance", "icon_failure_ttl_ms", "100", "99"),
        ("performance", "metadata_cache_limit", "16", "15"),
        ("performance", "metadata_ttl_ms", "100", "99"),
        ("performance", "icon_query_timeout_ms", "10", "9"),
        ("performance", "snapshot_budget_ms", "5", "501"),
        ("performance", "render_scale", "2", "3"),
        ("performance", "render_budget_mb", "8", "257"),
        ("browser", "chrome_user_data_dir", "custom/chrome", ""),
        ("browser", "edge_user_data_dir", "custom/edge", "bad\0dir"),
    ];
    for section in ["switch-windows", "switch-apps"] {
        cases.extend([
            (section, "include_topmost", "yes", "maybe"),
            (section, "include_tool_windows", "yes", "maybe"),
            (section, "include_untitled", "yes", "maybe"),
            (section, "min_width", "0", "4097"),
            (section, "min_height", "0", "4097"),
            (section, "exclude_titles", "", "bad\0title"),
            (section, "exclude_processes", "app.exe", "bad\0.exe"),
        ]);
    }
    let template = document::parse_ini(DEFAULT_CONFIG).unwrap();
    assert_eq!(SETTINGS.len(), 73);
    assert_eq!(cases.len(), SETTINGS.len());
    assert_eq!(
        template
            .iter()
            .map(|(_, properties)| properties.len())
            .sum::<usize>(),
        SETTINGS.len()
    );
    for setting in SETTINGS {
        let case = cases
            .iter()
            .find(|(section, key, ..)| *section == setting.section && *key == setting.key)
            .unwrap();
        assert!(!setting.consumer.is_empty());
        assert_eq!(
            template.get_from(
                (!setting.section.is_empty()).then_some(setting.section),
                setting.key
            ),
            Some(setting.default)
        );
        let parse = |value: &str| {
            let text = if setting.section.is_empty() {
                format!("{}={value}\n", setting.key)
            } else {
                format!("[{}]\n{}={value}\n", setting.section, setting.key)
            };
            document::parse_ini(&text).and_then(|ini| Config::load(&ini))
        };
        assert_ne!(
            parse(case.2).unwrap(),
            Config::default(),
            "unused {}.{}",
            case.0,
            case.1
        );
        assert!(
            parse(case.3).is_err(),
            "accepted invalid {}.{}",
            case.0,
            case.1
        );
    }
}

#[test]
fn nondefault_values_reach_hotkeys_desktop_filter_and_badge_consumers() {
    let config = Config::load(&document::parse_ini("[switch-windows]\nenable=no\nonly_current_desktop=no\n[switch-apps]\nenable=yes\nhotkey=ctrl+f12\nonly_current_desktop=yes\nbadge_max=250\nbadge_color=#123456\nbadge_text_color=#654321\nbadge_font_size=18\n").unwrap()).unwrap();
    let hotkeys = config.to_hotkeys();
    assert_eq!(hotkeys.len(), 1);
    assert_eq!(hotkeys[0].id, SWITCH_APPS_HOTKEY_ID);
    assert_eq!(hotkeys[0].modifier, [0x1d, 0x1d]);
    assert!(!config.switch_windows_only_current_desktop());
    assert!(config.switch_apps_only_current_desktop());
    let badge = crate::badge::BadgeStyle::from_config(&config);
    assert_eq!(
        (badge.background, badge.foreground, badge.font_size),
        (0x123456, 0x654321, 18)
    );
    assert_eq!(
        crate::badge::format_badge_count(251, config.switch_apps_badge_max).as_deref(),
        Some("250+")
    );
}

#[test]
fn stage_c_ini_sizes_and_process_exclusions_reach_their_consumers() {
    use crate::{
        keyboard::state::SwitchKind,
        layout::{LayoutOptions, LayoutSnapshot, MonitorSnapshot, PixelRect},
        window_snapshot::filter::WindowFilter,
    };
    let config = Config::load(&document::parse_ini("[appearance]\npanel_width=740\npanel_height=164\nicon_padding=6\npanel_padding=12\nitem_gap=8\nmax_columns=4\n[switch-apps]\nexclude_processes=EXCLUDED.EXE\n[switch-windows]\nexclude_processes=OTHER.EXE\n").unwrap()).unwrap();
    let screen = PixelRect {
        left: -1000,
        top: 0,
        right: 0,
        bottom: 600,
    };
    let layout = LayoutSnapshot::calculate(
        &LayoutOptions::from_config(&config),
        MonitorSnapshot {
            identity: 1,
            screen,
            available: screen,
            dpi: 96,
        },
        9,
        8,
        0,
    )
    .unwrap();
    assert_eq!(
        (
            layout.bounds.width(),
            layout.bounds.height(),
            layout.icon_size
        ),
        (740, 164, 128)
    );
    assert_eq!(
        (layout.capacity, layout.page, layout.items.len()),
        (4, 2, 1)
    );
    let item = &layout.items[0];
    assert_eq!(item.icon.left - item.outer.left, 6);
    assert_eq!(item.outer.left, 78);
    assert_eq!(
        layout.hit_test(
            layout.bounds.left + item.outer.left,
            layout.bounds.top + item.outer.top
        ),
        Some(8)
    );
    assert!(!WindowFilter::from_config(&config, SwitchKind::Apps).allows_process("excluded.exe"));
    assert!(WindowFilter::from_config(&config, SwitchKind::Windows).allows_process("excluded.exe"));
    assert!(!WindowFilter::from_config(&config, SwitchKind::Windows).allows_process("other.exe"));
}

#[test]
fn tray_write_preserves_custom_values_duplicates_comments_and_utf16() {
    use super::{
        encoding::{self, IniEncoding},
        settings::save_startup_enabled,
        test_support::TestDirectory,
    };
    use std::fs;
    let directory = TestDirectory::new();
    let original = "; 用户注释\r\n[switch-apps]\nenable=yes\r\nbadge_max=321\r\n[other]\nx=keep\r\n[startup]\nenabled = auto\r\nenabled = no\r\n";
    fs::write(
        directory.ini(),
        encoding::encode(original, IniEncoding::Utf16Le),
    )
    .unwrap();
    let expected = Config::load(&document::parse_ini(original).unwrap()).unwrap();
    save_startup_enabled(&directory.ini(), &expected, true).unwrap();
    let bytes = fs::read(directory.ini()).unwrap();
    let (updated, kind) = encoding::decode(&bytes).unwrap();
    assert_eq!(kind, IniEncoding::Utf16Le);
    assert!(updated.contains("; 用户注释\r\n"));
    assert!(updated.contains("badge_max=321\r\n"));
    assert!(updated.contains("x=keep\r\n"));
    assert!(updated.contains("enabled = yes\r\nenabled = no\r\n"));
    let configured = Config::load(&document::parse_ini(&updated).unwrap()).unwrap();
    assert!(configured.switch_apps_enable);
    assert_eq!(configured.startup_enabled, super::StartupEnabled::Yes);
    assert!(save_startup_enabled(&directory.ini(), &expected, false).is_err());
    assert_eq!(fs::read(directory.ini()).unwrap(), bytes);
}
