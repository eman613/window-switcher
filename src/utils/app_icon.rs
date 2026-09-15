//! Compatibility helpers for inspection tools. UI icon work lives in IconService.
use super::com::ComApartment;
use crate::{
    config::Config,
    icon_loader::{file, native, IconLoader},
};
use indexmap::IndexMap;
use std::{path::Path, time::Duration};
use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::HICON};

pub fn get_app_icon(
    override_icons: &IndexMap<String, String>,
    module_path: &str,
    hwnd: HWND,
) -> HICON {
    let config = Config {
        switch_apps_override_icons: override_icons.clone(),
        ..Default::default()
    };
    let icon = (|| {
        let _com = ComApartment::sta().ok()?;
        let directory = super::get_exe_folder().ok()?;
        IconLoader::new(&config, &directory)
            .native(module_path, hwnd, || true)
            .0
    })();
    icon.or_else(native::fallback)
        .map(native::OwnedIcon::into_raw)
        .unwrap_or_default()
}

pub fn load_image_as_hicon<T: AsRef<Path>>(image_path: T) -> Option<HICON> {
    file::load(image_path.as_ref()).map(native::OwnedIcon::into_raw)
}

pub fn get_window_icon(hwnd: HWND) -> Option<HICON> {
    native::window_icon(
        hwnd,
        Duration::from_millis(Config::default().icon_query_timeout_ms.into()),
    )
    .map(native::OwnedIcon::into_raw)
}
