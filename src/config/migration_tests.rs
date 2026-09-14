use std::{
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    document::{merge_missing, parse_ini},
    encoding::{decode, encode, IniEncoding},
    storage::{load_at, write_atomic},
    Config, DEFAULT_CONFIG,
};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        loop {
            let number = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "window-switcher-config-test-{}-{number}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("create test directory: {err}"),
            }
        }
    }

    fn config_path(&self) -> PathBuf {
        self.0.join("window-switcher.ini")
    }

    fn assert_no_temporary_files(&self) {
        assert!(fs::read_dir(&self.0).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("clean owned test directory");
    }
}

fn assert_complete(text: &str) {
    let actual = parse_ini(text).unwrap();
    for (section, properties) in &parse_ini(DEFAULT_CONFIG).unwrap() {
        for (key, _) in properties {
            assert!(
                actual.get_from(section, key).is_some(),
                "missing {section:?}.{key}"
            );
        }
    }
}

#[test]
fn defaults_and_chinese_template_agree() {
    assert_eq!(
        Config::load(&parse_ini(DEFAULT_CONFIG).unwrap()).unwrap(),
        Config::default()
    );
    assert_eq!(merge_missing(DEFAULT_CONFIG).unwrap(), DEFAULT_CONFIG);
    for (section, properties) in &parse_ini(DEFAULT_CONFIG).unwrap() {
        for (key, _) in properties {
            let name = section.map_or_else(|| key.to_owned(), |section| format!("{section}.{key}"));
            assert!(DEFAULT_CONFIG.contains(&format!("; 配置说明：{name}\n")));
        }
    }
}

#[test]
fn old_config_gains_missing_keys_without_resetting_custom_values() {
    let original = "; 用户注释\r\ntrayicon = no\r\ncustom_global = 保留\r\n[switch-apps]\r\nenable  = yes\r\nbadge_max = 250\r\ncustom_key = 保留=等号\r\n[custom]\r\npath = \\\\server\\share\\目录";
    let merged = merge_missing(original).unwrap();
    let config = Config::load(&parse_ini(&merged).unwrap()).unwrap();
    assert!(!config.trayicon);
    assert!(config.switch_apps_enable);
    assert_eq!(config.switch_apps_badge_max, 250);
    assert!(merged.contains("; 用户注释\r\n"));
    assert!(merged.contains("enable  = yes\r\n"));
    assert!(merged.contains("custom_key = 保留=等号\r\n"));
    assert!(merged.contains("[custom]\r\npath = \\\\server\\share\\目录"));
    assert!(!merged.ends_with('\n'));
    assert!(merged.contains("; 配置说明：switch-apps.enable\r\n"));
    assert_eq!(merge_missing(&merged).unwrap(), merged);
    assert_complete(&merged);
}

#[test]
fn duplicate_keys_and_sections_retain_original_precedence() {
    let original = "[switch-apps]\nenable = yes\nbadge_max = 42\nbadge_max = 73\n[switch-apps]\nenable = no\n[custom]\nx=1\nx=2\n";
    let merged = merge_missing(original).unwrap();
    assert_eq!(
        Config::load(&parse_ini(original).unwrap()).unwrap(),
        Config::load(&parse_ini(&merged).unwrap()).unwrap()
    );
    assert!(merged.contains("badge_max = 42\nbadge_max = 73\n"));
    assert!(merged.contains("[switch-apps]\nenable = no\n[custom]\nx=1\nx=2\n"));
    assert_eq!(
        merged.matches("; 配置说明：switch-apps.badge_max").count(),
        1
    );
    assert_eq!(merge_missing(&merged).unwrap(), merged);
}

#[test]
fn quoted_multiline_unknown_values_are_not_treated_as_sections_or_keys() {
    let original = "[custom]\nvalue = \"第一行\n[switch-apps]\nbadge_max = 3\n最后一行\"\n[switch-apps]\nenable = yes\n";
    let merged = merge_missing(original).unwrap();
    assert_eq!(
        parse_ini(original)
            .unwrap()
            .get_from(Some("custom"), "value"),
        parse_ini(&merged)
            .unwrap()
            .get_from(Some("custom"), "value")
    );
    let config = Config::load(&parse_ini(&merged).unwrap()).unwrap();
    assert!(config.switch_apps_enable);
    assert_eq!(config.switch_apps_badge_max, 99);
    assert_complete(&merged);
}

#[test]
fn missing_sections_do_not_capture_keys_of_an_existing_final_section() {
    for original in [
        "[log]\nlevel = warn\n",
        "[log]\npath =\n[switch-windows]\nhotkey = ctrl+f2\n",
        "[custom]\nx = 1\n",
    ] {
        let merged = merge_missing(original).unwrap();
        assert_complete(&merged);
        assert_eq!(
            Config::load(&parse_ini(original).unwrap()).unwrap(),
            Config::load(&parse_ini(&merged).unwrap()).unwrap()
        );
        assert_eq!(merge_missing(&merged).unwrap(), merged);
    }
}

#[test]
fn encodings_newlines_and_personalized_values_survive_real_file_migration() {
    for encoding in [
        IniEncoding::Utf8,
        IniEncoding::Utf8Bom,
        IniEncoding::Utf16Le,
        IniEncoding::Utf16Be,
    ] {
        for newline in ["\n", "\r\n"] {
            let directory = TestDirectory::new();
            let path = directory.config_path();
            let original = [
                "; 用户自己的说明",
                "[switch-apps]",
                "enable = yes",
                "badge_max = 123",
            ]
            .join(newline);
            let bytes = encode(&original, encoding);
            fs::write(&path, &bytes).unwrap();
            let loaded = load_at(path.clone()).unwrap();
            assert!(loaded.migrated);
            assert!(loaded.config.switch_apps_enable);
            assert_eq!(loaded.config.switch_apps_badge_max, 123);
            let migrated = fs::read(&path).unwrap();
            let (text, detected) = decode(&migrated).unwrap();
            assert_eq!(encode(&text, detected), migrated);
            assert_eq!(encode(&text, encoding), migrated);
            assert!(!text.ends_with('\n'));
            assert!(text.contains(&format!("; 用户自己的说明{newline}")));
            assert!(text.contains(&format!("; 配置说明：switch-apps.enable{newline}")));
            assert_complete(&text);
            assert!(!load_at(path.clone()).unwrap().migrated);
            assert_eq!(fs::read(&path).unwrap(), migrated);
            directory.assert_no_temporary_files();
        }
    }
}

#[test]
fn absent_file_is_created_once() {
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let loaded = load_at(path.clone()).unwrap();
    assert!(loaded.migrated);
    assert_eq!(loaded.config, Config::default());
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
    assert!(!load_at(path).unwrap().migrated);
    directory.assert_no_temporary_files();
}

#[test]
fn malformed_or_invalid_configuration_is_never_overwritten() {
    for original in [
        b"[broken".as_slice(),
        b"[switch-apps]\nenable = maybe\n",
        b"[switch-apps]\nbadge_max = 1\n",
        b"[log]\nlevel = nonsense\n",
        &[0xff, 0xfe, 0x01],
        &[0x80, 0x90],
    ] {
        let directory = TestDirectory::new();
        let path = directory.config_path();
        fs::write(&path, original).unwrap();
        assert!(load_at(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
        directory.assert_no_temporary_files();
    }
}

#[test]
fn concurrent_edit_or_creation_is_preserved_and_temporary_files_are_removed() {
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let edited = b"[switch-apps]\nenable = yes\nbadge_max = 237\n";
    fs::write(&path, edited).unwrap();
    assert!(write_atomic(&path, Some(b"older snapshot"), DEFAULT_CONFIG.as_bytes()).is_err());
    assert_eq!(fs::read(&path).unwrap(), edited);
    assert!(write_atomic(&path, None, DEFAULT_CONFIG.as_bytes()).is_err());
    assert_eq!(fs::read(path).unwrap(), edited);
    directory.assert_no_temporary_files();
}

#[test]
fn logging_cannot_overwrite_the_configuration_file() {
    let directory = TestDirectory::new();
    let path = directory.config_path();
    for log_path in [
        path.clone(),
        directory.0.join(".").join("window-switcher.ini"),
    ] {
        let original = format!(
            "[switch-apps]\nenable = yes\n[log]\npath = {}\n",
            log_path.display()
        );
        fs::write(&path, &original).unwrap();
        assert!(load_at(path.clone()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        directory.assert_no_temporary_files();
    }
}

#[test]
fn readonly_or_locked_configuration_cannot_be_reset() {
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let original = b"[switch-apps]\nenable = yes\n";
    fs::write(&path, original).unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let result = load_at(path.clone());
    fs::set_permissions(&path, original_permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    directory.assert_no_temporary_files();

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert!(load_at(path.clone()).is_err());
    drop(lock);
    assert_eq!(fs::read(&path).unwrap(), original);
    directory.assert_no_temporary_files();
}
