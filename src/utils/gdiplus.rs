use std::{marker::PhantomData, ptr, rc::Rc};

use anyhow::{bail, Result};
use windows::Win32::Graphics::GdiPlus::{
    GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, Status,
};

pub(crate) fn check_status(status: Status, operation: &str) -> Result<()> {
    if status.0 != 0 {
        bail!("gdiplus stage={operation} status={}", status.0);
    }
    Ok(())
}

pub(crate) struct GdiPlusRuntime {
    token: usize,
    _thread: PhantomData<Rc<()>>,
}

impl GdiPlusRuntime {
    pub(crate) fn new() -> Result<Self> {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let mut token = 0;
        check_status(
            unsafe { GdiplusStartup(&mut token, &input, ptr::null_mut()) },
            "startup",
        )?;
        if token == 0 {
            bail!("gdiplus stage=startup null token");
        }
        Ok(Self {
            token,
            _thread: PhantomData,
        })
    }
}

impl Drop for GdiPlusRuntime {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.token) };
    }
}

pub(crate) struct OwnedGp<T> {
    pub(crate) ptr: *mut T,
    destroy: unsafe fn(*mut T) -> Status,
}

impl<T> OwnedGp<T> {
    /// The factory and destructor must refer to the same GDI+ resource type.
    pub(crate) unsafe fn create(
        operation: &str,
        factory: impl FnOnce(*mut *mut T) -> Status,
        destroy: unsafe fn(*mut T) -> Status,
    ) -> Result<Self> {
        let mut output = ptr::null_mut();
        check_status(factory(&mut output), operation)?;
        if output.is_null() {
            bail!("gdiplus stage={operation} null output");
        }
        Ok(Self {
            ptr: output,
            destroy,
        })
    }
}

impl<T> Drop for OwnedGp<T> {
    fn drop(&mut self) {
        let status = unsafe { (self.destroy)(self.ptr) };
        if status.0 != 0 {
            warn!("gdiplus stage=release status={}", status.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_and_null_output_are_not_mistaken_for_success() {
        unsafe fn must_not_destroy(_: *mut u8) -> Status {
            panic!("invalid output must not be destroyed")
        }
        assert!(check_status(Status(2), "injected").is_err());
        assert!(unsafe { OwnedGp::create("failure", |_| Status(2), must_not_destroy) }.is_err());
        assert!(unsafe { OwnedGp::create("null", |_| Status(0), must_not_destroy) }.is_err());
    }
}
