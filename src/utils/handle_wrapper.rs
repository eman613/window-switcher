use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// Unique owner of a real CloseHandle-compatible handle, never a pseudo handle.
#[derive(Debug, Default)]
pub struct HandleWrapper {
    handle: HANDLE,
}

impl HandleWrapper {
    pub fn new(handle: HANDLE) -> Self {
        Self { handle }
    }
    pub fn get_handle(&self) -> HANDLE {
        self.handle
    }
    pub fn get_handle_mut(&mut self) -> &mut HANDLE {
        &mut self.handle
    }
}

impl Drop for HandleWrapper {
    fn drop(&mut self) {
        if self.handle.is_invalid() {
            return;
        }
        unsafe {
            if let Err(err) = CloseHandle(self.handle) {
                warn!("resource stage=close-handle code={:#x}", err.code().0);
            }
        }
    }
}
