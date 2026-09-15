use super::to_wstring;

use anyhow::{anyhow, bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::{
    Foundation::{
        CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE,
        WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
};

pub const INSTANCE_NAME: &str = "WindowSwitcherMutex";

/// A struct representing one running instance.
pub struct SingleInstance {
    handle: Option<HANDLE>,
    owned: bool,
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
        Ok(SingleInstance {
            owned: handle.is_some(),
            handle,
        })
    }

    /// Returns whether this instance is single.
    pub fn is_single(&self) -> bool {
        self.owned
    }

    /// Only a child with a private restart channel may open the existing object.
    /// Normal duplicate launches still close their handle immediately.
    pub fn replacement(name: &str) -> Result<Self> {
        let name = to_wstring(name);
        unsafe { SetLastError(ERROR_SUCCESS) };
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }?;
        let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let instance = Self {
            handle: Some(handle),
            owned: false,
        };
        if !existing {
            bail!("restart stage=mutex parent reservation unavailable");
        }
        Ok(instance)
    }

    pub fn acquire(&mut self) -> Result<()> {
        if self.owned {
            return Ok(());
        }
        let handle = self
            .handle
            .context("restart stage=mutex missing reservation")?;
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                self.owned = true;
                Ok(())
            }
            WAIT_TIMEOUT => bail!("restart stage=mutex input owner has not released the mutex"),
            _ => Err(windows::core::Error::from_win32()).context("restart stage=mutex acquire"),
        }
    }

    pub fn release(&mut self) -> Result<()> {
        if self.owned {
            unsafe { ReleaseMutex(self.handle.context("restart stage=mutex missing handle")?) }
                .context("restart stage=mutex release")?;
            self.owned = false;
        }
        Ok(())
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                if self.owned {
                    let _ = ReleaseMutex(handle);
                }
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

    #[test]
    fn handoff_reserves_the_name_and_allows_rollback_after_child_failure() {
        let name = format!("WindowSwitcherHandoffTest-{}", std::process::id());
        let mut original = SingleInstance::create(&name).unwrap();
        // Mutexes are recursive on one thread; the replacement must use another
        // thread, just as the production replacement is another process.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let child_name = name.clone();
        let thread = std::thread::spawn(move || {
            let mut replacement = SingleInstance::replacement(&child_name).unwrap();
            assert!(replacement.acquire().is_err());
            ready_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            replacement.acquire().unwrap();
            assert!(replacement.is_single());
        });
        ready_rx.recv().unwrap();
        original.release().unwrap();
        assert!(!SingleInstance::create(&name).unwrap().is_single());
        go_tx.send(()).unwrap();
        thread.join().unwrap();
        original.acquire().unwrap();
        assert!(original.is_single());
    }
}
