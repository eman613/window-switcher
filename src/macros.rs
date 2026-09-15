use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

use crate::utils::to_wstring;

pub fn message_box(text: &str) {
    let language = crate::localization::Text::current();
    message_box_with_title(
        language.error_title(),
        &language.failure(crate::localization::FailureKind::Launch, text),
    );
}

pub fn message_box_with_title(title: &str, text: &str) {
    let text = to_wstring(text);
    let title = to_wstring(title);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr() as _),
            PCWSTR(title.as_ptr()),
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
