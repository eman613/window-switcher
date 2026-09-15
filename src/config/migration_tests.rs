use std::{
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    document::{merge_missing, parse_ini},
    encoding::{decode, encode, IniEncoding},
    storage::load_at,
    transaction::write_preserving,
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
        if let Err(err) = fs::remove_dir_all(&self.0) {
            if std::thread::panicking() {
                eprintln!("owned test directory cleanup failed: {err}");
            } else {
                panic!("owned test directory cleanup failed: {err}");
            }
        }
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
            let comment = format!("; 配置说明：{name}");
            assert!(DEFAULT_CONFIG.lines().any(|line| line == comment));
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
    assert!(write_preserving(&path, Some(b"older snapshot"), DEFAULT_CONFIG.as_bytes()).is_err());
    assert_eq!(fs::read(&path).unwrap(), edited);
    assert!(write_preserving(&path, None, DEFAULT_CONFIG.as_bytes()).is_err());
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

#[test]
fn existing_empty_and_comment_only_files_never_become_defaults() {
    for text in ["", " \r\n", "; 保存中\n", "# 保留\n[switch-apps]\n"] {
        for encoding in [
            IniEncoding::Utf8,
            IniEncoding::Utf8Bom,
            IniEncoding::Utf16Le,
            IniEncoding::Utf16Be,
        ] {
            let directory = TestDirectory::new();
            let path = directory.config_path();
            let original = encode(text, encoding);
            fs::write(&path, &original).unwrap();
            assert!(load_at(path.clone()).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            directory.assert_no_temporary_files();
        }
    }
}

#[test]
fn input_and_encoded_output_size_are_both_limited() {
    use super::storage::MAX_INI_BYTES;
    for encoding in [IniEncoding::Utf8, IniEncoding::Utf16Le] {
        let directory = TestDirectory::new();
        let path = directory.config_path();
        let mut original = encode("[custom]\nx=1\n;", encoding);
        let unit = encode("a", encoding);
        let padding = if matches!(encoding, IniEncoding::Utf16Le) {
            &unit[2..]
        } else {
            &unit[..]
        };
        while original.len() + padding.len() <= MAX_INI_BYTES as usize - 32 {
            original.extend_from_slice(padding);
        }
        fs::write(&path, &original).unwrap();
        assert!(
            load_at(path.clone()).is_err(),
            "merge must not make an unreadable file"
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        let oversized = vec![b'a'; MAX_INI_BYTES as usize + 1];
        assert!(write_preserving(&path, Some(&original), &oversized).is_err());
        fs::write(&path, &oversized).unwrap();
        assert!(load_at(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), oversized);
        directory.assert_no_temporary_files();
    }
}

fn editor_replace(path: &std::path::Path, replacement: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS},
    };
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let source: Vec<u16> = replacement
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(source.as_ptr()),
            PCWSTR::null(),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
        .map_err(|error| {
            // This BOOL API wraps GetLastError in HRESULT_FROM_WIN32. The
            // generic windows::Error -> io::Error conversion retains HRESULT.
            std::io::Error::from_raw_os_error((error.code().0 as u32 & 0xffff) as i32)
        })?;
    }
    Ok(())
}

#[test]
fn commit_boundaries_preserve_all_editor_save_styles() {
    use super::transaction::{recovery_path, write_with_hook, CommitStage};
    let original = b"[custom]\nvalue=old\n";
    let latest = b"[custom]\nvalue=user-new\n";
    let merged = merge_missing(std::str::from_utf8(original).unwrap()).unwrap();
    for boundary in [
        CommitStage::Prepared,
        CommitStage::Checked,
        CommitStage::OriginalMoved,
        CommitStage::Published,
    ] {
        for operation in 0..4 {
            let directory = TestDirectory::new();
            let path = directory.config_path();
            let editor = directory.0.join("editor.ini");
            fs::write(&path, original).unwrap();
            fs::write(&editor, latest).unwrap();
            let result = write_with_hook(&path, Some(original), merged.as_bytes(), |stage| {
                if stage != boundary {
                    return Ok(());
                }
                let attempted = match operation {
                    0 => fs::write(&path, latest),
                    1 => fs::rename(&editor, &path),
                    2 => {
                        if stage == CommitStage::OriginalMoved {
                            fs::write(&path, original)?;
                        }
                        editor_replace(&path, &editor)
                    }
                    _ => fs::remove_file(&path)
                        .or_else(|err| {
                            if err.kind() == std::io::ErrorKind::NotFound {
                                Ok(())
                            } else {
                                Err(err)
                            }
                        })
                        .and_then(|()| fs::write(&path, latest)),
                };
                if matches!(stage, CommitStage::Checked | CommitStage::Published) {
                    let error = attempted.expect_err("the pinned target accepted an external save");
                    // MoveFileEx/ReplaceFile may report ACCESS_DENIED instead of
                    // SHARING_VIOLATION. The invariant is failure + unchanged bytes.
                    assert!(
                        matches!(error.raw_os_error(), Some(5 | 32)),
                        "boundary={stage:?} operation={operation} code={:?}",
                        error.raw_os_error()
                    );
                    assert_eq!(
                        fs::read(&path).unwrap(),
                        if stage == CommitStage::Checked {
                            original.as_slice()
                        } else {
                            merged.as_bytes()
                        }
                    );
                } else {
                    attempted.unwrap();
                }
                Ok(())
            });
            if matches!(boundary, CommitStage::Checked | CommitStage::Published) {
                result.unwrap();
                assert_eq!(fs::read(&path).unwrap(), merged.as_bytes());
                assert!(!recovery_path(&path).unwrap().exists());
            } else {
                assert!(result.is_err());
                assert_eq!(fs::read(&path).unwrap(), latest);
                if boundary == CommitStage::OriginalMoved {
                    assert_eq!(fs::read(recovery_path(&path).unwrap()).unwrap(), original);
                    assert!(load_at(path.clone()).is_err());
                }
            }
            directory.assert_no_temporary_files();
        }
    }
}

#[test]
fn publication_failure_restores_original_and_interrupted_state_never_creates_defaults() {
    use super::transaction::{recovery_path, write_with_hook, CommitStage};
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let original = b"[custom]\nvalue=keep\n";
    fs::write(&path, original).unwrap();
    let result = write_with_hook(&path, Some(original), DEFAULT_CONFIG.as_bytes(), |stage| {
        if stage == CommitStage::OriginalMoved {
            anyhow::bail!("injected publish failure");
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::rename(&path, recovery_path(&path).unwrap()).unwrap();
    assert!(load_at(path.clone()).is_err());
    assert!(!path.exists());
    assert_eq!(fs::read(recovery_path(&path).unwrap()).unwrap(), original);
    directory.assert_no_temporary_files();
}

#[test]
fn logging_checks_hardlinks_and_the_current_ini_on_every_write() {
    use super::prepare_log_file;
    use std::io::Write;
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let log = directory.0.join("app.log");
    fs::write(&path, DEFAULT_CONFIG).unwrap();
    fs::hard_link(&path, &log).unwrap();
    assert!(prepare_log_file(&log, &path).is_err());
    assert!(super::storage::validate_log_destination(&path, Some(&log)).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
    fs::remove_file(&log).unwrap();
    let mut sink = prepare_log_file(&log, &path).unwrap();
    assert!(
        prepare_log_file(&log, &path).is_ok(),
        "watcher preflight must allow the running log sink"
    );
    sink.write_all(b"anonymous event\n").unwrap();
    fs::remove_file(&path).unwrap();
    fs::hard_link(&log, &path).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(sink.write_all(b"must not pollute ini").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(sink);
}

#[test]
fn original_hardlink_identity_is_preserved_and_parent_rename_is_denied() {
    use super::{
        file_identity::PinnedPath,
        transaction::{write_with_hook, CommitStage},
    };
    let mut directory = TestDirectory::new();
    let path = directory.config_path();
    let alias = directory.0.join("original-alias.ini");
    let original = b"[custom]\nvalue=keep\n";
    fs::write(&path, original).unwrap();
    fs::hard_link(&path, &alias).unwrap();
    let merged = merge_missing(std::str::from_utf8(original).unwrap()).unwrap();
    write_with_hook(&path, Some(original), merged.as_bytes(), |stage| {
        if stage == CommitStage::Checked {
            assert_eq!(
                fs::write(&alias, b"changed").unwrap_err().raw_os_error(),
                Some(32)
            );
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(&alias).unwrap(), original);
    let _pin = PinnedPath::new(&path).unwrap();
    let renamed = directory.0.with_extension("renamed");
    let rename = fs::rename(&directory.0, &renamed);
    if rename.is_ok() {
        // Keep owning the fixture even when the assertion below detects a bug.
        directory.0 = renamed;
    }
    assert_eq!(rename.err().and_then(|err| err.raw_os_error()), Some(32));
}

#[test]
#[ignore = "requires Windows symlink privilege or Developer Mode"]
fn stage_a_reparse_log_alias_is_rejected_before_and_after_open() {
    use std::{io::Write, os::windows::fs::symlink_file};
    let directory = TestDirectory::new();
    let path = directory.config_path();
    let log = directory.0.join("app.log");
    fs::write(&path, DEFAULT_CONFIG).unwrap();
    symlink_file(&path, &log).expect("symlink fixture capability");
    assert!(super::prepare_log_file(&log, &path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
    fs::remove_file(&log).unwrap();
    let mut sink = super::prepare_log_file(&log, &path).unwrap();
    sink.write_all(b"anonymous\n").unwrap();
    fs::remove_file(&path).unwrap();
    symlink_file(&log, &path).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(sink.write_all(b"must not pollute ini").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn custom_permissions_and_alternate_streams_are_not_silently_discarded() {
    use windows::Win32::{Foundation::GENERIC_READ, Storage::FileSystem::WRITE_DAC};
    let original = b"[custom]\nvalue=private\n";
    let directory = TestDirectory::new();
    let path = directory.config_path();
    fs::write(&path, original).unwrap();
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ.0 | WRITE_DAC.0)
        .open(&path)
        .unwrap();
    super::metadata::change_test_dacl_protection(&file);
    drop(file);
    let error = load_at(path.clone())
        .err()
        .expect("custom permissions must reject replacement");
    assert!(format!("{error:#}").contains("权限"));
    assert_eq!(fs::read(&path).unwrap(), original);
    directory.assert_no_temporary_files();

    let directory = TestDirectory::new();
    let path = directory.config_path();
    fs::write(&path, original).unwrap();
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":retained");
    fs::write(&stream, b"retained metadata").unwrap();
    let error = load_at(path.clone())
        .err()
        .expect("additional streams must reject replacement");
    assert!(format!("{error:#}").contains("数据流"));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(&stream).unwrap(), b"retained metadata");
    directory.assert_no_temporary_files();
}

#[test]
fn ordinary_attributes_and_creation_time_survive_supplementation() {
    use std::{mem::size_of, os::windows::fs::MetadataExt};
    use windows::Win32::Storage::FileSystem::{
        FileBasicInfo, SetFileInformationByHandle, FILE_ATTRIBUTE_HIDDEN,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_BASIC_INFO,
    };
    let directory = TestDirectory::new();
    let path = directory.config_path();
    fs::write(&path, b"[custom]\nvalue=keep\n").unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let before = file.metadata().unwrap();
    let attributes =
        before.file_attributes() | FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0;
    let basic = FILE_BASIC_INFO {
        FileAttributes: attributes,
        ..Default::default()
    };
    unsafe {
        SetFileInformationByHandle(
            super::file_identity::handle(&file),
            FileBasicInfo,
            &basic as *const _ as _,
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .unwrap();
    drop(file);
    assert!(load_at(path.clone()).unwrap().migrated);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.creation_time(), before.creation_time());
    assert_eq!(after.file_attributes(), attributes);
    assert!(fs::read_to_string(&path).unwrap().contains("value=keep"));
    directory.assert_no_temporary_files();
}
