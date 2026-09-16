use super::layout::PreviewLayout;
use crate::{utils::window_identity::WindowIdentity, window_snapshot::lifetimes::WindowLifetimes};
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{E_INVALIDARG, HWND, RECT},
    Graphics::Dwm::*,
    UI::WindowsAndMessaging::{
        GetAncestor, GetWindowDisplayAffinity, IsIconic, IsWindowVisible, GA_ROOT, WDA_NONE,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placeholder {
    Unavailable,
    Minimized,
    Protected,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceStatus {
    Ready { affinity_known: bool },
    Placeholder(Placeholder),
}

pub(super) fn inspect(source: WindowIdentity, lifetimes: &WindowLifetimes) -> Result<SourceStatus> {
    if !source.is_current(lifetimes) {
        return Ok(SourceStatus::Placeholder(Placeholder::Closed));
    }
    let hwnd = source.hwnd();
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Ok(SourceStatus::Placeholder(Placeholder::Minimized));
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool()
        || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || !unsafe { DwmIsCompositionEnabled() }
            .context("preview stage=composition")?
            .as_bool()
    {
        return Ok(SourceStatus::Placeholder(Placeholder::Unavailable));
    }
    let mut affinity = 0;
    let affinity_known = unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.is_ok();
    if affinity_known && affinity != WDA_NONE.0 {
        return Ok(SourceStatus::Placeholder(Placeholder::Protected));
    }
    // A failed affinity query is unknown, never proof that capture is allowed.
    // Only DWM renders the source and continues to enforce OS content protection.
    Ok(SourceStatus::Ready { affinity_known })
}

pub(super) trait ThumbnailApi {
    fn register(&self, destination: HWND, source: HWND) -> windows::core::Result<isize>;
    fn unregister(&self, handle: isize) -> windows::core::Result<()>;
}

pub(super) struct DwmApi;
impl ThumbnailApi for DwmApi {
    fn register(&self, destination: HWND, source: HWND) -> windows::core::Result<isize> {
        unsafe { DwmRegisterThumbnail(destination, source) }
    }
    fn unregister(&self, handle: isize) -> windows::core::Result<()> {
        unsafe { DwmUnregisterThumbnail(handle) }
    }
}

/// Unknown unregister failures retain the lease and prevent a second registration.
pub(super) struct Thumbnail<A: ThumbnailApi = DwmApi> {
    api: A,
    handle: Option<isize>,
    registrations: u64,
    releases: u64,
    release_error: Option<i32>,
}

impl Default for Thumbnail {
    fn default() -> Self {
        Self::new(DwmApi)
    }
}

impl<A: ThumbnailApi> Thumbnail<A> {
    fn new(api: A) -> Self {
        Self {
            api,
            handle: None,
            registrations: 0,
            releases: 0,
            release_error: None,
        }
    }
    pub(super) fn active(&self) -> bool {
        self.handle.is_some()
    }
    pub(super) fn attach(&mut self, destination: HWND, source: HWND) -> Result<()> {
        ensure!(
            self.handle.is_none(),
            "preview stage=register previous-lease-retained"
        );
        let handle = self
            .api
            .register(destination, source)
            .context("preview stage=register")?;
        ensure!(handle != 0, "preview stage=register null-handle");
        self.handle = Some(handle);
        self.registrations = self.registrations.saturating_add(1);
        debug!(
            "preview stage=register active=1 registrations={} releases={}",
            self.registrations, self.releases
        );
        Ok(())
    }
    pub(super) fn release(&mut self) -> bool {
        let Some(handle) = self.handle else {
            return true;
        };
        let invalidated = match self.api.unregister(handle) {
            Ok(()) => false,
            // DWM invalidates the relationship when its source is destroyed.
            // The documented E_INVALIDARG result means this handle no longer
            // exists; retaining it would permanently block later selections.
            Err(error) if error.code() == E_INVALIDARG => true,
            Err(error) => {
                if self.release_error != Some(error.code().0) {
                    warn!(
                        "preview stage=unregister code={:#x} active=1; retaining lease",
                        error.code().0
                    );
                    self.release_error = Some(error.code().0);
                }
                return false;
            }
        };
        self.handle = None;
        self.release_error = None;
        self.releases = self.releases.saturating_add(1);
        debug!(
            "preview stage=unregister active=0 registrations={} releases={} invalidated={invalidated}",
            self.registrations, self.releases
        );
        true
    }
}

impl Thumbnail {
    pub(super) fn size(&self) -> Result<(i32, i32)> {
        let size = unsafe {
            DwmQueryThumbnailSourceSize(self.handle.context("preview stage=size no-lease")?)
        }
        .context("preview stage=source-size")?;
        ensure!(
            size.cx > 0 && size.cy > 0,
            "preview stage=source-size empty"
        );
        Ok((size.cx, size.cy))
    }
    pub(super) fn show(&self, layout: PreviewLayout) -> Result<()> {
        let content = layout.content;
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION
                | DWM_TNP_VISIBLE
                | DWM_TNP_OPACITY
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: RECT {
                left: content.left,
                top: content.top,
                right: content.right,
                bottom: content.bottom,
            },
            opacity: 255,
            fVisible: true.into(),
            fSourceClientAreaOnly: false.into(),
            ..Default::default()
        };
        unsafe {
            DwmUpdateThumbnailProperties(
                self.handle.context("preview stage=show no-lease")?,
                &properties,
            )
        }
        .context("preview stage=update")
    }
}

impl<A: ThumbnailApi> Drop for Thumbnail<A> {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Default)]
    struct RecordingApi {
        active: Rc<Cell<u32>>,
        fail_release: Rc<Cell<bool>>,
    }
    impl ThumbnailApi for RecordingApi {
        fn register(&self, _: HWND, _: HWND) -> windows::core::Result<isize> {
            assert_eq!(self.active.replace(1), 0);
            Ok(1)
        }
        fn unregister(&self, _: isize) -> windows::core::Result<()> {
            if self.fail_release.get() {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )));
            }
            if self.active.get() == 0 {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            }
            assert_eq!(self.active.replace(0), 1);
            Ok(())
        }
    }

    #[test]
    fn system_invalidated_relationship_allows_the_next_selection() {
        let api = RecordingApi::default();
        let mut thumbnail = Thumbnail::new(api.clone());
        thumbnail.attach(HWND::default(), HWND::default()).unwrap();
        api.active.set(0); // Source destruction already removed the relationship.
        assert!(thumbnail.release());
        assert!(thumbnail.release());
        assert!(!thumbnail.active());
        assert_eq!(thumbnail.releases, 1);
        thumbnail.attach(HWND::default(), HWND::default()).unwrap();
        assert_eq!(api.active.get(), 1);
        assert!(thumbnail.release());
        assert_eq!(thumbnail.registrations, thumbnail.releases);
    }

    #[test]
    fn failed_release_keeps_ownership_and_blocks_replacement() {
        let api = RecordingApi::default();
        let mut thumbnail = Thumbnail::new(api.clone());
        thumbnail.attach(HWND::default(), HWND::default()).unwrap();
        api.fail_release.set(true);
        assert!(!thumbnail.release());
        assert!(thumbnail.active());
        assert!(thumbnail.attach(HWND::default(), HWND::default()).is_err());
        assert_eq!(api.active.get(), 1);
        api.fail_release.set(false);
        assert!(thumbnail.release());
        assert!(thumbnail.release());
        for _ in 0..256 {
            thumbnail.attach(HWND::default(), HWND::default()).unwrap();
            assert!(thumbnail.release());
        }
        assert_eq!(thumbnail.registrations, thumbnail.releases);
        thumbnail.attach(HWND::default(), HWND::default()).unwrap();
        drop(thumbnail);
        assert_eq!(api.active.get(), 0);
    }
}
