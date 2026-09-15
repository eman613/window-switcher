use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};

use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

use super::file_identity::{require_regular, FileIdentity, PinnedPath};

mod rotation;
mod worker;
pub(crate) use worker::initialize_logging;

struct LogFailure {
    recorded: AtomicBool,
    pending: AtomicBool,
    code: AtomicI32,
}

impl LogFailure {
    const fn new() -> Self {
        Self {
            recorded: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            code: AtomicI32::new(0),
        }
    }

    fn record(&self, error: &io::Error) {
        if self
            .recorded
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.code
                .store(error.raw_os_error().unwrap_or(0), Ordering::Relaxed);
            self.pending.store(true, Ordering::Release);
        }
    }

    fn take(&self) -> Option<i32> {
        self.pending
            .swap(false, Ordering::AcqRel)
            .then(|| self.code.load(Ordering::Relaxed))
    }
}

static LOG_FAILURE: LogFailure = LogFailure::new();

// The logger ignores Write errors. Report the first one from the UI timer,
// outside the logger's mutex. A failed diagnostic cannot recursively notify.
pub(crate) fn take_log_failure() -> Option<i32> {
    LOG_FAILURE.take()
}

/// Each append pins the current INI namespace and denies concurrent replacement
/// for the duration of the identity check + write. Editors remain free between
/// appends. Never log an error from inside this sink (the logger owns a mutex).
pub struct GuardedLogFile {
    file: File,
    identity: FileIdentity,
    ini_path: PathBuf,
}

pub fn prepare_log_file(path: &Path, ini_path: &Path) -> io::Result<GuardedLogFile> {
    let ini = PinnedPath::new(ini_path)?;
    let original = open_ini_guard(&ini.path)?;
    let log = PinnedPath::new(path)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&log.path)?;
    require_regular(&file)?;
    let identity = FileIdentity::read(&file)?;
    reject_alias(identity, FileIdentity::read(&original)?)?;
    Ok(GuardedLogFile {
        file,
        identity,
        ini_path: ini.path.clone(),
    })
}

pub(super) fn validate_destination(ini_path: &Path, log_path: &Path) -> io::Result<()> {
    let ini = PinnedPath::new(ini_path)?;
    let log = PinnedPath::new(log_path)?;
    if ini
        .path
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&log.path.as_os_str().to_string_lossy())
    {
        return Err(alias_error());
    }
    let existing_log = match OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&log.path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    require_regular(&existing_log)?;
    let original = match open_ini_guard(&ini.path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    reject_alias(
        FileIdentity::read(&existing_log)?,
        FileIdentity::read(&original)?,
    )
}

fn open_ini_guard(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    require_regular(&file)?;
    Ok(file)
}

fn alias_error() -> io::Error {
    io::Error::other("日志与 INI 指向同一文件；请使用独立日志文件，未写入日志")
}

fn reject_alias(log: FileIdentity, ini: FileIdentity) -> io::Result<()> {
    if log == ini {
        Err(alias_error())
    } else {
        Ok(())
    }
}

impl Write for GuardedLogFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = (|| {
            let pinned = PinnedPath::new(&self.ini_path)?;
            let original = open_ini_guard(&pinned.path)?;
            reject_alias(self.identity, FileIdentity::read(&original)?)?;
            self.file.write(bytes)
        })();
        if let Err(error) = &result {
            LOG_FAILURE.record(error);
        }
        result
    }

    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        // simple-logging writes a whole record with write!; validate once per record.
        self.write_all(fmt::format(args).as_bytes())
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.file.flush();
        if let Err(error) = &result {
            LOG_FAILURE.record(error);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_errors_are_observable_once_without_recursive_logging() {
        let failure = LogFailure::new();
        assert_eq!(failure.take(), None);
        failure.record(&io::Error::from_raw_os_error(32));
        assert_eq!(failure.take(), Some(32));
        failure.record(&io::Error::other("notification logging also failed"));
        assert_eq!(failure.take(), None);
    }
}
