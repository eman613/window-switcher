use std::{os::windows::ffi::OsStrExt, path::Path};

use anyhow::{Context, Result};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Storage::FileSystem::{
            FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        },
        System::Threading::WaitForSingleObject,
    },
};

pub(super) struct DirectoryNotification(HANDLE);

impl DirectoryNotification {
    pub(super) fn new(path: &Path) -> Result<Self> {
        let directory: Vec<_> = path
            .parent()
            .context("INI 缺少目录")?
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        Ok(Self(
            unsafe {
                FindFirstChangeNotificationW(
                    PCWSTR(directory.as_ptr()),
                    false,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE,
                )
            }
            .context("无法建立 INI 目录通知")?,
        ))
    }

    pub(super) fn changed(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.0, 0) } {
            WAIT_OBJECT_0 => {
                unsafe { FindNextChangeNotification(self.0) }
                    .context("INI 目录通知重新挂起失败")?;
                Ok(true)
            }
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_win32()).context("INI 目录通知失效"),
        }
    }
}

impl Drop for DirectoryNotification {
    fn drop(&mut self) {
        if let Err(error) = unsafe { FindCloseChangeNotification(self.0) } {
            warn!("config stage=notification-close code={:#x}", error.code().0);
        }
    }
}
