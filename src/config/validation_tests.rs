use super::{document::parse_ini, Config, DEFAULT_BADGE_COLOR, DEFAULT_BADGE_FONT_SIZE};

#[test]
fn every_new_setting_is_read_and_range_checked() {
    let config = Config::load(&parse_ini("auto_restart = no\nrestart_delay_ms = 2500\n[switch-apps]\nbadge_color = #91a2B3\nbadge_text_color = 102030\nbadge_font_size = 10\nbadge_max = 987\n").unwrap()).unwrap();
    assert!(!config.auto_restart);
    assert_eq!(config.restart_delay_ms, 2500);
    assert_eq!(config.switch_apps_badge_color, 0x91a2b3);
    assert_eq!(config.switch_apps_badge_text_color, 0x102030);
    assert_eq!(config.switch_apps_badge_font_size, 10);
    assert_eq!(config.switch_apps_badge_max, 987);

    for invalid in [
        "auto_restart = maybe\n",
        "restart_delay_ms = 199\n",
        "restart_delay_ms = 10001\n",
        "[switch-apps]\nbadge_font_size = 7\n",
        "[switch-apps]\nbadge_font_size = 25\n",
        "[switch-apps]\nbadge_color = #123\n",
        "[switch-apps]\nbadge_color = #GGFFFF\n",
        "[switch-apps]\nbadge_text_color = #FFFFFFFF\n",
        "[switch-apps]\nenable = invalid\n",
        "[switch-windows]\nonly_current_desktop = maybe\n",
        "[log]\nlevel = verbose\n",
        "[switch-apps]\noverride_icons = =icon.ico\n",
        "[switch-apps]\noverride_icons = app.exe=\n",
    ] {
        assert!(
            Config::load(&parse_ini(invalid).unwrap()).is_err(),
            "accepted {invalid}"
        );
    }
    let defaults = Config::default();
    assert_eq!(defaults.switch_apps_badge_color, DEFAULT_BADGE_COLOR);
    assert_eq!(
        defaults.switch_apps_badge_font_size,
        DEFAULT_BADGE_FONT_SIZE
    );
}

#[test]
fn badge_shape_and_size_preserve_defaults_and_reject_invalid_values() {
    use super::BadgeShape;
    let defaults = Config::default();
    assert_eq!(defaults.switch_apps_badge_shape, BadgeShape::Circle);
    assert_eq!(defaults.switch_apps_badge_size, None);
    for (shape, expected_shape) in [
        ("circle", BadgeShape::Circle),
        ("square", BadgeShape::Square),
    ] {
        for (size, expected_size) in [
            ("auto", None),
            ("16", Some(16)),
            ("24", Some(24)),
            ("48", Some(48)),
        ] {
            let ini = parse_ini(&format!(
                "[switch-apps]\nbadge_shape={shape}\nbadge_size={size}\n"
            ))
            .unwrap();
            let config = Config::load(&ini).unwrap();
            assert_eq!(config.switch_apps_badge_shape, expected_shape);
            assert_eq!(config.switch_apps_badge_size, expected_size);
            assert_eq!(config.switch_apps_badge_font_size, DEFAULT_BADGE_FONT_SIZE);
        }
    }
    for (key, values) in [
        ("badge_shape", &["", "round", "ellipse", "rectangle"][..]),
        (
            "badge_size",
            &[
                "",
                "0",
                "15",
                "49",
                "-1",
                "16.5",
                "999999999999999999999",
                "large",
            ][..],
        ),
    ] {
        for value in values {
            let ini = parse_ini(&format!("[switch-apps]\n{key}={value}\n")).unwrap();
            let error = Config::load(&ini).unwrap_err();
            assert!(
                format!("{error:#}").contains(key),
                "missing key in diagnostic: {error:#}"
            );
        }
    }
}

#[test]
fn windows_and_unc_paths_remain_literal() {
    let ini = parse_ini("[log]\npath = \\\\server\\share\\logs\\switcher.log\n[switch-apps]\noverride_icons = app.exe=\\\\server\\share\\icon.png,other.exe=D:\\Icons\\other.ico\n").unwrap();
    let config = Config::load(&ini).unwrap();
    assert_eq!(
        config.log_file.unwrap().to_string_lossy(),
        r"\\server\share\logs\switcher.log"
    );
    assert_eq!(
        config.switch_apps_override_icons["app.exe"],
        r"\\server\share\icon.png"
    );
    assert_eq!(
        config.switch_apps_override_icons["other.exe"],
        r"D:\Icons\other.ico"
    );
}

#[test]
fn empty_blacklist_and_icon_entries_do_not_match_every_application() {
    let config = Config::load(
        &parse_ini("[switch-windows]\nblacklist = , ,\n[switch-apps]\noverride_icons = , ;\n")
            .unwrap(),
    )
    .unwrap();
    assert!(config.switch_windows_blacklist.is_empty());
    assert!(config.switch_apps_override_icons.is_empty());
}

#[test]
fn supported_aliases_and_empty_hotkeys_remain_compatible() {
    for (alias, enabled) in [
        ("yes", true),
        ("true", true),
        ("on", true),
        ("1", true),
        ("no", false),
        ("false", false),
        ("off", false),
        ("0", false),
    ] {
        let config = Config::load(
            &parse_ini(&format!(
                "trayicon = {alias}\n[switch-apps]\nhotkey =\nenable = {alias}\n"
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(config.trayicon, enabled);
        assert_eq!(config.switch_apps_enable, enabled);
        assert_eq!(
            config.switch_apps_hotkey,
            Config::default().switch_apps_hotkey
        );
    }
}
