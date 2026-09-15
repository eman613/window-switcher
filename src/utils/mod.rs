mod admin;
mod app_icon;
pub(crate) mod appx;
pub(crate) mod browser;
mod check_error;
pub(crate) mod com;
pub(crate) mod command;
pub(crate) mod gdi;
mod handle_wrapper;
mod regedit;
pub(crate) mod scheduled_task;
mod single_instance;
mod token;
mod window;
pub(crate) mod window_identity;
mod windows_theme;
mod windows_version;

pub use admin::*;
pub use app_icon::*;
pub use check_error::*;
pub use handle_wrapper::*;
pub use regedit::*;
pub use scheduled_task::*;
pub use single_instance::*;
pub use window::*;
pub use windows_theme::*;
pub use windows_version::*;

pub fn to_wstring(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>()
}
