use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    fs::OpenOptions,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use anyhow::{anyhow, Context, Result};
use ini::{Ini, ParseOption};

use crate::{
    config::Config,
    config_diagnostics::{prepare_config, ConfigDiagnostic, ConfigDiagnosticSeverity},
    metrics::StageTimer,
    utils::{get_exe_folder, get_system_command_path},
};

const CONFIG_FILE_NAME: &str = "window-switcher.ini";
const LEGACY_CONFIG_FILE_NAME: &str = "windows-switcher.ini";
const CONFIG_DIRECTORY_NAME: &str = "WindowSwitcher";
const DEFAULT_CONFIG: &str = include_str!("../window-switcher.ini");
const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFileStamp {
    exists: bool,
    modified: Option<SystemTime>,
    length: u64,
    content_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    pub path: PathBuf,
    pub stamp: ConfigFileStamp,
}

#[derive(Debug)]
pub struct ConfigLoadReport {
    pub config: Config,
    pub source: ConfigSource,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

impl ConfigLoadReport {
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_warning())
            .count()
    }
}

pub fn load_config() -> Result<Config> {
    Ok(load_config_report()?.config)
}

pub fn load_config_report() -> Result<ConfigLoadReport> {
    let _timer = StageTimer::new("config_load");
    let path = resolve_config_path()?;
    load_config_from_path(&path)
}

pub(crate) fn load_config_from_path(path: &Path) -> Result<ConfigLoadReport> {
    let before = config_file_stamp(path)?;
    if !before.exists {
        return Ok(ConfigLoadReport {
            config: Config::default(),
            source: ConfigSource {
                path: path.to_path_buf(),
                stamp: before,
            },
            diagnostics: Vec::new(),
        });
    }

    let source = read_config_source(path)?;
    let after = config_file_stamp(path)?;
    if before != after {
        return Err(anyhow!(
            "Config file changed while it was being read: '{}'",
            path.display()
        ));
    }

    let source = source.strip_prefix('\u{feff}').unwrap_or(&source);
    let option = ParseOption {
        enabled_escape: false,
        ..Default::default()
    };
    let mut ini = Ini::load_from_str_opt(source, option)
        .map_err(|err| anyhow!("Failed to parse config file '{}', {err}", path.display()))?;
    let mut diagnostics = prepare_config(source, &mut ini);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(LEGACY_CONFIG_FILE_NAME))
    {
        diagnostics.push(ConfigDiagnostic {
            severity: ConfigDiagnosticSeverity::Warning,
            line: None,
            message: format!(
                "过时配置文件名 / deprecated file name {LEGACY_CONFIG_FILE_NAME}; use {CONFIG_FILE_NAME}"
            ),
        });
    }
    let mut config = Config::load(&ini)?;
    config.resolve_relative_paths(path.parent());

    Ok(ConfigLoadReport {
        config,
        source: ConfigSource {
            path: path.to_path_buf(),
            stamp: after,
        },
        diagnostics,
    })
}

pub(crate) fn current_config_source() -> Result<ConfigSource> {
    let path = resolve_config_path()?;
    let stamp = config_file_stamp(&path)?;
    Ok(ConfigSource { path, stamp })
}

pub(crate) fn config_file_stamp(path: &Path) -> Result<ConfigFileStamp> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(ConfigFileStamp {
            exists: true,
            modified: metadata.modified().ok(),
            length: metadata.len(),
            content_hash: hash_config_file(path, metadata.len())?,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFileStamp {
            exists: false,
            modified: None,
            length: 0,
            content_hash: 0,
        }),
        Err(err) => {
            Err(err).with_context(|| format!("Failed to inspect config file '{}'", path.display()))
        }
    }
}

fn read_config_source(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("Failed to read config file '{}'", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_FILE_BYTES {
        return Err(anyhow!(
            "Config file '{}' exceeds the {}-byte safety limit",
            path.display(),
            MAX_CONFIG_FILE_BYTES
        ));
    }
    String::from_utf8(bytes)
        .map_err(|err| anyhow!("Config file '{}' is not valid UTF-8: {err}", path.display()))
}

fn hash_config_file(path: &Path, length: u64) -> Result<u64> {
    if length > MAX_CONFIG_FILE_BYTES {
        return Err(anyhow!(
            "Config file '{}' exceeds the {}-byte safety limit",
            path.display(),
            MAX_CONFIG_FILE_BYTES
        ));
    }
    let bytes = fs::read(path)
        .with_context(|| format!("Failed to read config file '{}'", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_FILE_BYTES {
        return Err(anyhow!(
            "Config file '{}' exceeds the {}-byte safety limit",
            path.display(),
            MAX_CONFIG_FILE_BYTES
        ));
    }
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

pub(crate) fn edit_config_file(path: &Path) -> Result<()> {
    debug!("open config file '{}'", path.display());
    ensure_default_config(path)?;
    let notepad_path = get_system_command_path("notepad.exe")?;
    Command::new(notepad_path)
        .arg(path)
        .spawn()
        .map_err(|err| anyhow!("Failed to open config file '{}', {err}", path.display()))?;
    Ok(())
}

fn resolve_config_path() -> Result<PathBuf> {
    let exe_folder = get_exe_folder()?;
    Ok(select_config_path(
        &exe_folder,
        env::var_os("LOCALAPPDATA").map(PathBuf::from).as_deref(),
    ))
}

fn select_config_path(exe_folder: &Path, local_app_data: Option<&Path>) -> PathBuf {
    let portable = exe_folder.join(CONFIG_FILE_NAME);
    if portable.is_file() {
        return portable;
    }
    let legacy_portable = exe_folder.join(LEGACY_CONFIG_FILE_NAME);
    if legacy_portable.is_file() {
        return legacy_portable;
    }
    match local_app_data {
        Some(folder) => folder.join(CONFIG_DIRECTORY_NAME).join(CONFIG_FILE_NAME),
        None => portable,
    }
}

fn ensure_default_config(path: &Path) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory '{}'", parent.display()))?;
    }

    for attempt in 0..16u32 {
        let temporary = temporary_config_path(path, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let mut guard = TemporaryConfigGuard::new(temporary);
                file.write_all(DEFAULT_CONFIG.as_bytes()).with_context(|| {
                    format!("Failed to write default config file '{}'", path.display())
                })?;
                file.sync_all().with_context(|| {
                    format!("Failed to flush default config file '{}'", path.display())
                })?;
                drop(file);
                if path.is_file() {
                    return Ok(());
                }
                fs::rename(guard.path(), path).with_context(|| {
                    format!("Failed to install default config file '{}'", path.display())
                })?;
                guard.commit();
                return Ok(());
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to create default config file '{}'", path.display())
                });
            }
        }
    }

    Err(anyhow!(
        "Failed to allocate a temporary config file for '{}'",
        path.display()
    ))
}

fn temporary_config_path(path: &Path, attempt: u32) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        attempt
    ))
}

struct TemporaryConfigGuard {
    path: PathBuf,
    committed: bool,
}

impl TemporaryConfigGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for TemporaryConfigGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "window-switcher-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn portable_and_legacy_paths_take_precedence_over_user_directory() {
        let exe = TestDirectory::new("exe");
        let user = TestDirectory::new("user");

        assert_eq!(
            select_config_path(&exe.0, Some(&user.0)),
            user.0.join(CONFIG_DIRECTORY_NAME).join(CONFIG_FILE_NAME)
        );
        let legacy = exe.0.join(LEGACY_CONFIG_FILE_NAME);
        fs::write(&legacy, "trayicon = yes").unwrap();
        assert_eq!(select_config_path(&exe.0, Some(&user.0)), legacy);
        let portable = exe.0.join(CONFIG_FILE_NAME);
        fs::write(&portable, "trayicon = yes").unwrap();
        assert_eq!(select_config_path(&exe.0, Some(&user.0)), portable);
    }

    #[test]
    fn load_report_preserves_utf8_and_reports_line_numbers() {
        let directory = TestDirectory::new("load");
        let path = directory.0.join(CONFIG_FILE_NAME);
        fs::write(
            &path,
            "config_version = 1\ntrayicon = wrong\n[localization]\nlanguage = zh-CN\n",
        )
        .unwrap();

        let report = load_config_from_path(&path).unwrap();
        assert_eq!(report.config.language, crate::localization::Language::ZhCn);
        assert_eq!(report.warning_count(), 1);
        assert_eq!(report.diagnostics[0].line, Some(2));
    }

    #[test]
    fn relative_private_font_path_resolves_from_active_config_directory() {
        let directory = TestDirectory::new("font-path");
        let path = directory.0.join(CONFIG_FILE_NAME);
        fs::write(
            &path,
            "[appearance]\napp_name_font_file = fonts\\custom.ttf\n",
        )
        .unwrap();

        let report = load_config_from_path(&path).unwrap();
        assert_eq!(
            report.config.appearance.app_name_font_file,
            Some(directory.0.join("fonts").join("custom.ttf"))
        );
    }

    #[test]
    fn default_config_is_created_without_leaving_temporary_files() {
        let directory = TestDirectory::new("create");
        let path = directory.0.join("nested").join(CONFIG_FILE_NAME);

        ensure_default_config(&path).unwrap();

        assert!(path.is_file());
        let temporary_files = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[test]
    fn oversized_config_is_rejected_before_parsing() {
        let directory = TestDirectory::new("oversized");
        let path = directory.0.join(CONFIG_FILE_NAME);
        fs::write(&path, vec![b'x'; MAX_CONFIG_FILE_BYTES as usize + 1]).unwrap();

        let error = load_config_from_path(&path).unwrap_err();
        assert!(error.to_string().contains("safety limit"));
    }

    #[test]
    fn content_changes_invalidate_the_file_stamp() {
        let directory = TestDirectory::new("stamp");
        let path = directory.0.join(CONFIG_FILE_NAME);
        fs::write(&path, "trayicon = yes").unwrap();
        let first = config_file_stamp(&path).unwrap();
        fs::write(&path, "trayicon = no ").unwrap();
        let second = config_file_stamp(&path).unwrap();

        assert_ne!(first, second);
    }
}
