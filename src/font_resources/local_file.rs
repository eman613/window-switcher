//! Bounded local font reads. Keep the same handle from metadata through the copy.
use anyhow::{ensure, Context, Result};
use std::{
    fs::OpenOptions,
    io::Read,
    os::windows::fs::OpenOptionsExt,
    path::{Component, Path, Prefix},
};
use windows::{
    core::HSTRING,
    Win32::Storage::FileSystem::{GetDriveTypeW, FILE_SHARE_READ},
};

const MAX_FONT_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn read(path: &Path, ini_dir: &Path) -> Result<Vec<u8>> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        ini_dir.join(path)
    };
    ensure!(
        path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("ttf") || extension.eq_ignore_ascii_case("otf")
        }),
        "font stage=file unsupported-format"
    );
    require_local(&path)?;
    let path = path.canonicalize().context("font stage=file resolve")?;
    require_local(&path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(path)
        .context("font stage=file open")?;
    let metadata = file.metadata().context("font stage=file metadata")?;
    ensure!(
        metadata.is_file() && (1..=MAX_FONT_BYTES).contains(&metadata.len()),
        "font stage=file size"
    );
    let mut data = Vec::new();
    data.try_reserve_exact(metadata.len() as usize)
        .context("font stage=file allocation")?;
    file.take(MAX_FONT_BYTES + 1)
        .read_to_end(&mut data)
        .context("font stage=file read")?;
    ensure!(
        !data.is_empty() && data.len() as u64 <= MAX_FONT_BYTES,
        "font stage=file changed-size"
    );
    Ok(data)
}

fn require_local(path: &Path) -> Result<()> {
    let drive = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => anyhow::bail!("font stage=file local-drive-required"),
        },
        _ => anyhow::bail!("font stage=file absolute-path-required"),
    };
    let root = HSTRING::from(format!("{}:\\", char::from(drive)));
    ensure!(
        matches!(unsafe { GetDriveTypeW(&root) }, 2 | 3 | 5 | 6),
        "font stage=file remote-drive-refused"
    );
    Ok(())
}
