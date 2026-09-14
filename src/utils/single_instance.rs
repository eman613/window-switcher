use super::to_wstring;

use anyhow::{anyhow, Result};
use windows::core::PCWSTR;
use windows::Win32::{
    Foundation::{
        CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE,
    },
    System::Threading::{CreateMutexW, ReleaseMutex},
};

/// A struct representing one running instance.
pub struct SingleInstance {
    handle: Option<HANDLE>,
}

impl SingleInstance {
    /// Returns a new SingleInstance object.
    pub fn create(name: &str) -> Result<Self> {
        let name = to_wstring(name);
        unsafe { SetLastError(ERROR_SUCCESS) };
        let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
            .map_err(|err| anyhow!("Fail to setup single instance, {err}"))?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = if already_exists {
            unsafe { CloseHandle(handle) }
                .map_err(|err| anyhow!("Failed to close duplicate instance mutex, {err}"))?;
            None
        } else {
            Some(handle)
        };
        Ok(SingleInstance { handle })
    }

    /// Returns whether this instance is single.
    pub fn is_single(&self) -> bool {
        self.handle.is_some()
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                let _ = ReleaseMutex(handle);
                let _ = CloseHandle(handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SingleInstance;

    #[test]
    fn duplicate_instance_does_not_keep_the_mutex_alive_after_owner_exits() {
        let name = format!("WindowSwitcherMutexTest-{}", std::process::id());
        let first = SingleInstance::create(&name).unwrap();
        assert!(first.is_single());
        let second = SingleInstance::create(&name).unwrap();
        assert!(!second.is_single());
        drop(first);
        let replacement = SingleInstance::create(&name).unwrap();
        assert!(replacement.is_single());
        drop(second);
    }
}
