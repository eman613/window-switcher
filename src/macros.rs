use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

use crate::{
    localization::{text as localized_text, TextId},
    utils::to_wstring,
};

pub fn message_box(text: &str) {
    let text = to_wstring(text);
    let title = to_wstring(localized_text(TextId::ErrorTitle));
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr() as _),
            PCWSTR(title.as_ptr() as _),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[macro_export]
macro_rules! alert {
    ($($arg:tt)*) => {
        $crate::macros::message_box(&format!($($arg)*))
    };
}
