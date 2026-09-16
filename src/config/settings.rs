use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{document, encoding, storage, transaction, Config};

pub(crate) fn save_startup_enabled(path: &Path, expected: &Config, enabled: bool) -> Result<()> {
    save_boolean(path, "startup", "enabled", enabled, |current| {
        if current.startup_enabled != expected.startup_enabled
            || current.startup_run_level != expected.startup_run_level
            || current.startup_battery_policy != expected.startup_battery_policy
            || current.startup_command_timeout_ms != expected.startup_command_timeout_ms
        {
            bail!("自启动配置已被外部修改；请等待最新配置生效后再操作");
        }
        Ok(())
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
    let original = storage::read_bytes(path)?;
    let (text, encoding) = encoding::decode(&original)?;
    let current = Config::load(&document::parse_ini(&text)?)?;
    validate(&current)?;
    let text = document::set_value(&text, section, key, if value { "yes" } else { "no" })?;
    Config::load(&document::parse_ini(&text)?)?;
    transaction::write_preserving(path, Some(&original), &encoding::encode(&text, encoding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{encoding::IniEncoding, test_support::TestDirectory};
    use std::fs;

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
