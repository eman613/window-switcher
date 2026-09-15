use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ},
};

use super::{
    file_identity::{delete_owned, rename_no_replace, require_regular, FileIdentity, PinnedPath},
    metadata::PreservedMetadata,
    storage::{check_size, read_limited},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommitStage {
    Prepared,
    Checked,
    OriginalMoved,
    Published,
}

pub(super) fn recovery_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .context("INI 缺少文件名")?
        .to_string_lossy();
    Ok(path.with_file_name(format!(".{name}.recovery")))
}

pub(super) fn require_no_recovery(path: &Path) -> Result<()> {
    match fs::symlink_metadata(recovery_path(path)?) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context("无法核验 INI 恢复文件"),
        Ok(_) => bail!("发现未完成的 INI 补全；原值保留在同目录 .window-switcher.ini.recovery。已停止写回及默认配置创建，请先恢复该事务"),
    }
}

pub(super) fn write_preserving(path: &Path, expected: Option<&[u8]>, bytes: &[u8]) -> Result<()> {
    write_with_hook(path, expected, bytes, |_| Ok(()))
}

/// Windows cannot ReplaceFile while denying delete sharing. Move the locked
/// original by its own handle, then publish WITHOUT replacement. A concurrent
/// creator wins; rollback also never overwrites it. The recovery file remains
/// on interrupted/conflicting commits, so a later launch cannot invent defaults.
pub(super) fn write_with_hook(
    path: &Path,
    expected: Option<&[u8]>,
    bytes: &[u8],
    mut stage: impl FnMut(CommitStage) -> Result<()>,
) -> Result<()> {
    check_size(bytes)?;
    let pinned = PinnedPath::new(path).context("无法固定 INI 目录")?;
    let path = &pinned.path;
    require_no_recovery(path)?;
    let mut temporary = TemporaryConfig::create(path)?;
    // Reserve an empty file first. Never copy private INI bytes before checking
    // that the candidate has the same owner, permissions and integrity label.
    stage(CommitStage::Prepared)?;

    let original = if let Some(expected) = expected {
        let original = OpenOptions::new()
            .access_mode(GENERIC_READ.0 | DELETE.0)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .context("INI 正被写入、重命名或无法锁定；补全已取消")?;
        require_regular(&original)?;
        FileIdentity::read(&original)?;
        if original.metadata()?.permissions().readonly() {
            bail!("INI 为只读文件；原文件未修改");
        }
        if read_limited(&original)? != expected {
            bail!("检测到 INI 在补全期间被外部修改；已取消保存，请重试");
        }
        let metadata = PreservedMetadata::capture(&original, &temporary.file)?;
        Some((original, metadata))
    } else {
        None
    };
    temporary
        .file
        .write_all(bytes)
        .context("写入 INI 临时文件失败")?;
    if let Some((original, metadata)) = &original {
        metadata.apply(original, &temporary.file)?;
    }
    temporary.file.sync_all().context("同步 INI 临时文件失败")?;

    if let Some((original, _)) = original {
        stage(CommitStage::Checked)?;
        rename_no_replace(&original, &recovery_path(path)?)
            .context("无法保留 INI 原文件；补全已取消")?;
        let publish = stage(CommitStage::OriginalMoved)
            .and_then(|()| rename_no_replace(&temporary.file, path).context("INI 发布冲突或失败"));
        if let Err(err) = publish {
            return match rename_no_replace(&original, path) {
                Ok(()) => Err(err.context("INI 发布已取消，原文件已恢复")),
                Err(restore) => Err(err.context(format!(
                    "INI 原文件保留为恢复文件，未覆盖并发保存的目标；restore_code={:?}",
                    restore.raw_os_error()
                ))),
            };
        }
        temporary.published = true;
        stage(CommitStage::Published)?;
        delete_owned(&original).context("INI 已补全，但恢复文件清理失败；原值仍被保留")?;
    } else {
        // No replacement flag and no truncating create, including first launch.
        require_no_recovery(path)?;
        rename_no_replace(&temporary.file, path)
            .context("INI 已被其他程序创建或无法发布；未覆盖目标")?;
        temporary.published = true;
        stage(CommitStage::Published)?;
    }
    Ok(())
}

struct TemporaryConfig {
    file: File,
    published: bool,
}

impl TemporaryConfig {
    fn create(path: &Path) -> Result<Self> {
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
                .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        published: false,
                    })
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err).context("无法创建 INI 临时文件"),
            }
        }
        bail!("无法创建唯一的 INI 临时文件，请稍后重试")
    }
}

impl Drop for TemporaryConfig {
    fn drop(&mut self) {
        if !self.published {
            if let Err(err) = delete_owned(&self.file) {
                warn!("config stage=cleanup error_code={:?}", err.raw_os_error());
            }
        }
    }
}
