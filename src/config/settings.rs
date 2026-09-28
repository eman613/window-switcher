use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{
    document, encoding, quick::QuickSetting, reload, storage, transaction, Config, LoadedConfig,
    RunLevel,
};

pub(crate) fn read_current(path: &Path) -> Result<LoadedConfig> {
    transaction::require_no_recovery(path)?;
    reload::load_snapshot(&storage::read_bytes(path)?, path)
}

pub(crate) fn save_quick(
    path: &Path,
    expected: &Config,
    setting: QuickSetting,
) -> Result<LoadedConfig> {
    let (section, key) = setting.key();
    save_value(path, section, key, |current| {
        if setting.value(current) != setting.value(expected) {
            bail!("此设置已被外部修改；请先应用已保存设置后重试");
        }
        if let QuickSetting::StartupLevel(level) = setting {
            if current.startup_enabled != expected.startup_enabled
                || current.startup_battery_policy != expected.startup_battery_policy
                || current.startup_command_timeout_ms != expected.startup_command_timeout_ms
            {
                bail!("自启动配置已被外部修改；系统入口未修改");
            }
            if level == RunLevel::Highest
                && level != current.startup_run_level
                && !crate::utils::is_running_as_admin()?
            {
                bail!("请先以管理员身份重新启动，再设置最高权限自启动");
            }
        }
        setting.changed_value(current)
    })
    .context("快速设置未能保存；当前运行设置保留")
}

pub(crate) fn save_startup_enabled(
    path: &Path,
    expected: &Config,
    enabled: bool,
) -> Result<LoadedConfig> {
    save_value(path, "startup", "enabled", |current| {
        if current.startup_enabled != expected.startup_enabled
            || current.startup_run_level != expected.startup_run_level
            || current.startup_battery_policy != expected.startup_battery_policy
            || current.startup_command_timeout_ms != expected.startup_command_timeout_ms
        {
            bail!("自启动配置已被外部修改；请等待最新配置生效后再操作");
        }
        Ok(if enabled { "yes" } else { "no" }.to_owned())
    })
    .context("自启动设置未能保存；系统入口未修改")
}

pub(crate) fn save_input_paused(path: &Path, expected: &Config, paused: bool) -> Result<()> {
    save_boolean(path, "input", "paused", paused, |current| {
        if current.input_paused != expected.input_paused
            || current.pause_hotkey != expected.pause_hotkey
            || current.trayicon != expected.trayicon
        {
            bail!("暂停或恢复入口配置已被外部修改；请等待最新配置生效后再操作");
        }
        Ok(())
    })
    .context("暂停设置未能保存；当前输入状态未改变")
}

fn save_boolean(
    path: &Path,
    section: &str,
    key: &str,
    value: bool,
    validate: impl FnOnce(&Config) -> Result<()>,
) -> Result<()> {
    save_value(path, section, key, |current| {
        validate(current)?;
        Ok(if value { "yes" } else { "no" }.to_owned())
    })
    .map(|_| ())
}

fn save_value(
    path: &Path,
    section: &str,
    key: &str,
    select_value: impl FnOnce(&Config) -> Result<String>,
) -> Result<LoadedConfig> {
    transaction::require_no_recovery(path)?;
    let original = storage::read_bytes(path)?;
    let (text, encoding) = encoding::decode(&original)?;
    let current = Config::load(&document::parse_ini(&text)?)?;
    let value = select_value(&current)?;
    let text = document::set_value(&text, section, key, &value)?;
    let bytes = encoding::encode(&text, encoding);
    let loaded = reload::load_snapshot(&bytes, path)?;
    if bytes != original {
        transaction::write_preserving(path, Some(&original), &bytes)?;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{encoding::IniEncoding, test_support::TestDirectory};
    use std::fs;

    #[test]
    fn quick_settings_preserve_encodings_unknown_content_and_detect_external_conflicts() {
        let source = "; 用户注释\r\n[appearance]\r\ntheme = auto\r\napp_name_mode = selected\r\n[unknown]\r\nvalue = keep\r\n";
        let source = document::merge_missing(source).unwrap();
        for kind in [
            IniEncoding::Utf8,
            IniEncoding::Utf8Bom,
            IniEncoding::Utf16Le,
            IniEncoding::Utf16Be,
        ] {
            let directory = TestDirectory::new();
            let original = encoding::encode(&source, kind);
            fs::write(directory.ini(), &original).unwrap();
            let expected = read_current(&directory.ini()).unwrap();
            let setting = QuickSetting::Theme(super::super::Theme::Dark);
            let saved = save_quick(&directory.ini(), &expected.config, setting).unwrap();
            assert_eq!(saved.contents, fs::read(directory.ini()).unwrap());
            let (text, after_kind) = encoding::decode(&saved.contents).unwrap();
            assert_eq!(after_kind, kind);
            assert_eq!(text, source.replace("theme = auto", "theme = dark"));
            assert_eq!(expected.config.theme, super::super::Theme::Auto);
            let external = text.replace("value = keep", "value = external");
            fs::write(directory.ini(), encoding::encode(&external, kind)).unwrap();
            let changed = save_quick(&directory.ini(), &saved.config, QuickSetting::Names).unwrap();
            assert!(encoding::decode(&changed.contents)
                .unwrap()
                .0
                .contains("value = external"));
            let conflict = external.replace("theme = dark", "theme = light");
            let conflict = encoding::encode(&conflict, kind);
            fs::write(directory.ini(), &conflict).unwrap();
            assert!(save_quick(&directory.ini(), &saved.config, setting).is_err());
            assert_eq!(fs::read(directory.ini()).unwrap(), conflict);
        }
    }

    #[test]
    fn last_search_field_and_unresolved_recovery_never_write() {
        let directory = TestDirectory::new();
        let source = b"[search]\nfields=title\n";
        fs::write(directory.ini(), source).unwrap();
        let expected = read_current(&directory.ini()).unwrap();
        assert!(save_quick(
            &directory.ini(),
            &expected.config,
            QuickSetting::Field(super::super::SearchField::Title)
        )
        .is_err());
        assert_eq!(fs::read(directory.ini()).unwrap(), source);
        let recovery = transaction::recovery_path(&directory.ini()).unwrap();
        fs::write(&recovery, b"preserve recovery").unwrap();
        assert!(read_current(&directory.ini()).is_err());
        assert!(save_quick(&directory.ini(), &expected.config, QuickSetting::Names).is_err());
        assert_eq!(fs::read(directory.ini()).unwrap(), source);
        assert_eq!(fs::read(recovery).unwrap(), b"preserve recovery");
    }

    #[test]
    fn pause_round_trip_preserves_encodings_duplicates_and_unrelated_values() {
        let source = "; 用户注释\r\n[other]\r\nx = keep\n[input]\r\npaused = no\r\npaused = no\r\npause_hotkey = ctrl+f10\r\n";
        let source = document::merge_missing(source).unwrap();
        for kind in [
            IniEncoding::Utf8,
            IniEncoding::Utf8Bom,
            IniEncoding::Utf16Le,
            IniEncoding::Utf16Be,
        ] {
            let directory = TestDirectory::new();
            let original = encoding::encode(&source, kind);
            fs::write(directory.ini(), &original).unwrap();
            let expected = Config::load(&document::parse_ini(&source).unwrap()).unwrap();
            save_input_paused(&directory.ini(), &expected, true).unwrap();
            let saved = fs::read(directory.ini()).unwrap();
            let (text, saved_kind) = encoding::decode(&saved).unwrap();
            assert_eq!(saved_kind, kind);
            assert_eq!(text, source.replacen("paused = no", "paused = yes", 1));
            assert!(save_input_paused(&directory.ini(), &expected, false).is_err());
            assert_eq!(fs::read(directory.ini()).unwrap(), saved);
            let paused = Config::load(&document::parse_ini(&text).unwrap()).unwrap();
            save_input_paused(&directory.ini(), &paused, false).unwrap();
            assert_eq!(fs::read(directory.ini()).unwrap(), original);
        }
    }

    #[test]
    fn external_recovery_changes_and_missing_recovery_reject_pause_without_writing() {
        let directory = TestDirectory::new();
        let source = "trayicon=no\n[input]\npaused=no\npause_hotkey=ctrl+f10\n";
        let expected = Config::load(&document::parse_ini(source).unwrap()).unwrap();
        let edited = source.replace("ctrl+f10", "ctrl+f9");
        fs::write(directory.ini(), &edited).unwrap();
        assert!(save_input_paused(&directory.ini(), &expected, true).is_err());
        assert_eq!(fs::read_to_string(directory.ini()).unwrap(), edited);
        let no_recovery = source.replace("ctrl+f10", "");
        fs::write(directory.ini(), &no_recovery).unwrap();
        let expected = Config::load(&document::parse_ini(&no_recovery).unwrap()).unwrap();
        assert!(save_input_paused(&directory.ini(), &expected, true).is_err());
        assert_eq!(fs::read_to_string(directory.ini()).unwrap(), no_recovery);
    }
}
