use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{bail, Context, Result};
use windows::{
    core::PCWSTR,
    Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, MOVEFILE_WRITE_THROUGH,
        REPLACE_FILE_FLAGS,
    },
};

use super::{document, encoding, Config, LoadedConfig, DEFAULT_CONFIG};

const MAX_INI_BYTES: u64 = 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn prepare_log_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

pub(super) fn load_at(path: PathBuf) -> Result<LoadedConfig> {
    let original = match read_bytes(&path) {
        Ok(bytes) => Some(bytes),
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(err) => return Err(err.context(format!("config stage=read path={}", path.display()))),
    };
    let (text, encoding) =
        encoding::decode(original.as_deref().unwrap_or(DEFAULT_CONFIG.as_bytes()))
            .with_context(|| format!("config stage=decode path={}", path.display()))?;
    // Invalid user values are rejected before any file is written.
    parse_config(&text)
        .with_context(|| format!("config stage=validate path={}", path.display()))?;
    let merged = document::merge_missing(&text)
        .with_context(|| format!("config stage=merge path={}", path.display()))?;
    let config = parse_config(&merged)?;
    validate_log_destination(&path, config.log_file.as_deref())?;
    let contents = encoding::encode(&merged, encoding);
    let migrated = original.as_deref() != Some(contents.as_slice());
    if migrated {
        write_atomic(&path, original.as_deref(), &contents).with_context(|| {
            format!("config stage=save path={}；原配置不会重置", path.display())
        })?;
    }
    Ok(LoadedConfig {
        config,
        path,
        contents,
        migrated,
    })
}

pub(super) fn parse_config(text: &str) -> Result<Config> {
    Config::load(&document::parse_ini(text)?)
}

pub(super) fn validate_log_destination(ini_path: &Path, log_path: Option<&Path>) -> Result<()> {
    let Some(log_path) = log_path else {
        return Ok(());
    };
    let ini_path = fs::canonicalize(ini_path).unwrap_or_else(|_| ini_path.to_path_buf());
    let log_path = fs::canonicalize(log_path).unwrap_or_else(|_| log_path.to_path_buf());
    if ini_path
        .to_string_lossy()
        .eq_ignore_ascii_case(&log_path.to_string_lossy())
    {
        bail!("[log] path 不能指向 INI 文件本身；请使用独立日志文件，原配置未修改");
    }
    Ok(())
}

pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    read_limited(File::open(path)?)
}

fn read_limited(file: File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_INI_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INI_BYTES {
        bail!("INI 超过 1 MiB，已停止读取；请缩小文件后重试");
    }
    Ok(bytes)
}

/// Only a missing file gets defaults. Existing files use a checked atomic replacement.
pub(super) fn write_atomic(path: &Path, expected: Option<&[u8]>, bytes: &[u8]) -> Result<()> {
    let (temporary, mut file) = create_temporary(path)?;
    file.write_all(bytes).context("写入 INI 临时文件失败")?;
    file.sync_all().context("同步 INI 临时文件失败")?;
    drop(file);

    let target_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let temporary_wide: Vec<u16> = temporary
        .0
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if let Some(expected) = expected {
        // Deny in-place writers while validating and replacing. Allow delete sharing for ReplaceFileW.
        let guard = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
            .open(path)
            .context("INI 正被写入或无法读取；稍后重新启动即可补全")?;
        if guard.metadata()?.permissions().readonly() {
            bail!("INI 为只读文件；请取消只读后重新启动，原文件未修改");
        }
        if read_limited(guard.try_clone()?)? != expected || read_bytes(path)? != expected {
            bail!("检测到 INI 在补全期间被外部修改；已取消保存，请重试");
        }
        unsafe {
            ReplaceFileW(
                PCWSTR(target_wide.as_ptr()),
                PCWSTR(temporary_wide.as_ptr()),
                PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
        }
        .context("原子替换 INI 失败；请检查目录权限或编辑器占用")?;
    } else {
        // No REPLACE_EXISTING: a concurrently created user file must not be overwritten.
        unsafe {
            MoveFileExW(
                PCWSTR(temporary_wide.as_ptr()),
                PCWSTR(target_wide.as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        }
        .context("创建 INI 失败；文件可能已被其他程序创建，请重试")?;
    }
    Ok(())
}

fn create_temporary(path: &Path) -> Result<(TemporaryConfig, File)> {
    let name = path
        .file_name()
        .context("INI 缺少文件名")?
        .to_string_lossy();
    for _ in 0..16 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate =
            path.with_file_name(format!(".{name}.{}.{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((TemporaryConfig(candidate), file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err).context("无法在 INI 所在目录创建临时文件"),
        }
    }
    bail!("无法创建唯一的 INI 临时文件，请稍后重试")
}

struct TemporaryConfig(PathBuf);

impl Drop for TemporaryConfig {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.0) {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!("config stage=cleanup path={} error={err}", self.0.display());
            }
        }
    }
}
