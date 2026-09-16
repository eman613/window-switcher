use std::{marker::PhantomData, rc::Rc};

use anyhow::{Context, Result};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};

/// An apartment belongs to the calling thread, including S_FALSE acquisitions.
pub(crate) struct ComApartment(PhantomData<Rc<()>>);

impl ComApartment {
    pub(crate) fn sta() -> Result<Self> {
        Self::initialize(COINIT_APARTMENTTHREADED)
    }
    pub(crate) fn mta() -> Result<Self> {
        Self::initialize(COINIT_MULTITHREADED)
    }
    fn initialize(mode: COINIT) -> Result<Self> {
        unsafe { CoInitializeEx(None, mode) }
            .ok()
            .context("shell stage=com-initialize")?;
        Ok(Self(PhantomData))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_sta_acquisition_is_balanced_on_its_thread() {
        std::thread::spawn(|| {
            let first = ComApartment::sta().unwrap();
            let second = ComApartment::sta().unwrap();
            drop(second);
            drop(first);
            let _next = ComApartment::sta().unwrap();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn apartment_mode_conflict_does_not_uninitialize_an_unowned_reference() {
        use windows::Win32::System::Com::COINIT_MULTITHREADED;
        std::thread::spawn(|| {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .unwrap();
            assert!(ComApartment::sta().is_err());
            assert!(ComApartment::sta().is_err());
            unsafe { CoUninitialize() };
            let _sta = ComApartment::sta().unwrap();
        })
        .join()
        .unwrap();
    }
}
