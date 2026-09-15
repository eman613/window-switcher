use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0},
        Storage::FileSystem::{
            ReOpenFile, DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_WRITE_DATA,
        },
        System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
    },
};

use super::{open_ini_guard, prepare_log_file, reject_alias, GuardedLogFile};
use crate::config::file_identity::{delete_owned, require_regular, FileIdentity, PinnedPath};

const MAX_ARCHIVES: u32 = 20;
const HEADER_LIMIT: u64 = 256;

pub(super) struct RotatingLog {
    log: GuardedLogFile,
    truncate_file: File,
    path: PathBuf,
    mutex: OwnedHandle,
    max_bytes: u64,
    retained: u32,
    inspected: bool,
}

impl RotatingLog {
    pub(super) fn new(path: &Path, ini: &Path, max_bytes: u64, retained: u32) -> io::Result<Self> {
        if max_bytes < HEADER_LIMIT * 2 || retained > MAX_ARCHIVES {
            return Err(io::Error::other("日志限制无效"));
        }
        let log = prepare_log_file(path, ini)?;
        // Windows append-only handles cannot truncate. Reopen the verified file
        // object itself, never its mutable pathname, for bounded rotation.
        let truncate_handle = unsafe {
            ReOpenFile(
                HANDLE(log.file.as_raw_handle()),
                FILE_WRITE_DATA.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                Default::default(),
            )
        }?;
        let truncate_file = unsafe { File::from_raw_handle(truncate_handle.0) };
        let name = crate::utils::to_wstring(&format!(
            "Local\\WindowSwitcherLog-{}",
            log.identity.archive_key()
        ));
        let mutex = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }?;
        Ok(Self {
            log,
            truncate_file,
            path: path.to_owned(),
            mutex: unsafe { OwnedHandle::from_raw_handle(mutex.0) },
            max_bytes,
            retained,
            inspected: false,
        })
    }

    pub(super) fn record(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() as u64 > self.max_bytes {
            return Err(io::Error::other("单条日志超过大小限制"));
        }
        let mutex = HANDLE(self.mutex.as_raw_handle());
        if !matches!(
            unsafe { WaitForSingleObject(mutex, 0) },
            WAIT_OBJECT_0 | WAIT_ABANDONED
        ) {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let _held = MutexRelease(mutex);
        // The INI namespace and identity stay pinned through archive deletion,
        // copying, truncation and append, just as for the original guarded sink.
        let ini = PinnedPath::new(&self.log.ini_path)?;
        let original = open_ini_guard(&ini.path)?;
        let ini_identity = FileIdentity::read(&original)?;
        reject_alias(self.log.identity, ini_identity)?;
        let pinned = PinnedPath::new(&self.path)?;
        let length = self.log.file.metadata()?.len();
        if !self.inspected || length.saturating_add(bytes.len() as u64) > self.max_bytes {
            let archives = self.archives(&pinned.path, ini_identity)?;
            self.inspected = true;
            if length.saturating_add(bytes.len() as u64) > self.max_bytes {
                self.rotate(&pinned.path, archives, length, ini_identity)?;
            }
        }
        self.log.file.write_all(bytes)?;
        Ok(())
    }

    fn archive_path(path: &Path, index: u32) -> io::Result<PathBuf> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("日志缺少文件名"))?
            .to_string_lossy();
        Ok(path.with_file_name(format!("{name}.window-switcher.{index}.log")))
    }

    fn archives(&self, path: &Path, ini: FileIdentity) -> io::Result<Vec<Archive>> {
        let prefix = format!(
            "# window-switcher archive v1 {} ",
            self.log.identity.archive_key()
        );
        let mut archives = Vec::new();
        for index in 1..=MAX_ARCHIVES {
            let file = match OpenOptions::new()
                .access_mode(GENERIC_READ.0 | DELETE.0)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(Self::archive_path(path, index)?)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            require_regular(&file)?;
            reject_alias(FileIdentity::read(&file)?, ini)?;
            let mut header = Vec::new();
            (&file).take(HEADER_LIMIT).read_to_end(&mut header)?;
            let header = String::from_utf8_lossy(&header);
            let sequence = header
                .lines()
                .next()
                .and_then(|line| line.strip_prefix(&prefix))
                .and_then(|number| number.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::other("历史日志路径被非本应用文件占用；未覆盖或删除该文件")
                })?;
            if index > self.retained {
                delete_owned(&file)?;
            } else {
                archives.push(Archive {
                    file,
                    index,
                    sequence,
                });
            }
        }
        Ok(archives)
    }

    fn rotate(
        &mut self,
        path: &Path,
        mut archives: Vec<Archive>,
        length: u64,
        ini: FileIdentity,
    ) -> io::Result<()> {
        if self.retained > 0 {
            let sequence = archives
                .iter()
                .map(|archive| archive.sequence)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| io::Error::other("历史日志序号已用尽"))?;
            let index = match (1..=self.retained)
                .find(|index| archives.iter().all(|archive| archive.index != *index))
            {
                Some(index) => index,
                None => {
                    let oldest = archives
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, archive)| archive.sequence)
                        .unwrap()
                        .0;
                    let archive = archives.swap_remove(oldest);
                    delete_owned(&archive.file)?;
                    archive.index
                }
            };
            let mut archive = NewArchive {
                file: OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
                    .share_mode(FILE_SHARE_READ.0)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                    .open(Self::archive_path(path, index)?)?,
                complete: false,
            };
            require_regular(&archive.file)?;
            reject_alias(FileIdentity::read(&archive.file)?, ini)?;
            let header = format!(
                "# window-switcher archive v1 {} {sequence}\n",
                self.log.identity.archive_key()
            );
            archive.file.write_all(header.as_bytes())?;
            // Existing oversized logs are bounded too: retain their newest tail.
            let tail = length.min(self.max_bytes - HEADER_LIMIT);
            self.log.file.seek(SeekFrom::Start(length - tail))?;
            io::copy(&mut (&self.log.file).take(tail), &mut archive.file)?;
            archive.file.sync_all()?;
            archive.complete = true;
        }
        self.truncate_file.set_len(0)?;
        self.log.file.seek(SeekFrom::Start(0))?;
        Ok(())
    }
}

struct Archive {
    file: File,
    index: u32,
    sequence: u64,
}
struct NewArchive {
    file: File,
    complete: bool,
}
impl Drop for NewArchive {
    fn drop(&mut self) {
        if !self.complete {
            let _ = delete_owned(&self.file);
        }
    }
}
struct MutexRelease(HANDLE);
impl Drop for MutexRelease {
    fn drop(&mut self) {
        let _ = unsafe { ReleaseMutex(self.0) };
    }
}

#[cfg(test)]
mod tests;
