use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

use super::{
    document, encoding,
    file_identity::{require_regular, PinnedPath},
    logging,
    transaction::{require_no_recovery, write_preserving},
    Config, LoadedConfig, DEFAULT_CONFIG,
};

pub(super) const MAX_INI_BYTES: u64 = 1024 * 1024;

pub(super) fn load_at(path: PathBuf) -> Result<LoadedConfig> {
    let pinned = PinnedPath::new(&path).context("config stage=directory")?;
    require_no_recovery(&pinned.path)?;
    let original = match read_bytes(&path) {
        Ok(bytes) => Some(bytes),
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(err) => return Err(err.context("config stage=read")),
    };
    let (text, encoding) =
        encoding::decode(original.as_deref().unwrap_or(DEFAULT_CONFIG.as_bytes()))
            .context("config stage=decode")?;
    let parsed = document::parse_ini(&text)?;
    if original.is_some() && parsed.iter().all(|(_, properties)| properties.is_empty()) {
        bail!(
            "现有 INI 为空白、仅注释或无配置值；可能正在保存。已停止启动，原文件不会被默认值覆盖"
        );
    }
    // Invalid user values are rejected before any file is written.
    Config::load(&parsed).context("config stage=validate")?;
    let merged = document::merge_missing(&text).context("config stage=merge")?;
    let config = parse_config(&merged)?;
    validate_log_destination(&path, config.log_file.as_deref())?;
    let contents = encoding::encode(&merged, encoding);
    check_size(&contents).context("config stage=merged-size；原文件未修改")?;
    let migrated = original.as_deref() != Some(contents.as_slice());
    if migrated {
        write_preserving(&pinned.path, original.as_deref(), &contents)
            .context("config stage=save；原配置不会重置")?;
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
    logging::validate_destination(ini_path, log_path).context("config stage=log-destination")
}

pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let pinned = PinnedPath::new(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&pinned.path)?;
    require_regular(&file)?;
    read_limited(&file)
}

pub(super) fn read_limited(file: &File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_INI_BYTES + 1).read_to_end(&mut bytes)?;
    check_size(&bytes)?;
    Ok(bytes)
}

pub(super) fn check_size(bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_INI_BYTES {
        bail!("INI 超过 1 MiB，已停止读写；请缩小文件后重试");
    }
    Ok(())
}
