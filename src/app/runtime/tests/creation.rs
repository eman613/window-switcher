use super::*;
use crate::app::NAME;
use windows::Win32::{
    Foundation::{ERROR_CANNOT_FIND_WND_CLASS, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{RegisterClassW, UnregisterClassW, WNDCLASSW},
};

fn assert_window_retires(window: ApplicationWindow) {
    let hwnd = window.0;
    let target = window.target();
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
    assert!(target.is_live());
    drop(window);
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    assert!(!target.is_live());
    assert!(!target.try_post(WM_INPUT_READY));
    assert_eq!(Arc::strong_count(&target), 1);
}

#[test]
#[ignore = "requires a fresh isolated test process: mutates only its own native window class"]
fn native_class_registration_and_window_creation_failures_allow_recovery() {
    let module = HINSTANCE(unsafe { GetModuleHandleW(None) }.unwrap().0);
    let class = WNDCLASSW {
        hInstance: module,
        lpszClassName: NAME,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };

    // The actual RegisterClassW must reject a name already owned by this process.
    // This case must run before the factory's process-wide OnceCell is initialized.
    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
    let failure = ApplicationWindow::create().err();
    unsafe { UnregisterClassW(NAME, Some(module)) }.unwrap();
    let failure = failure.expect("duplicate class registration unexpectedly succeeded");
    assert!(failure.to_string().contains("ui stage=register-class"));
    assert_eq!(
        failure
            .downcast_ref::<windows::core::Error>()
            .unwrap()
            .code(),
        ERROR_CLASS_ALREADY_EXISTS.to_hresult()
    );
    assert_window_retires(ApplicationWindow::create().unwrap());

    // Keep the successful factory cache, but remove its native class. This makes
    // the real CreateWindowExW fail without changing production code or other apps.
    unsafe { UnregisterClassW(NAME, Some(module)) }.unwrap();
    let failure = ApplicationWindow::create().err();
    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
    let failure = failure.expect("creation without a registered class unexpectedly succeeded");
    assert!(failure.to_string().contains("ui stage=create-window"));
    assert_eq!(
        failure
            .downcast_ref::<windows::core::Error>()
            .unwrap()
            .code(),
        ERROR_CANNOT_FIND_WND_CLASS.to_hresult()
    );
    assert_window_retires(ApplicationWindow::create().unwrap());
    unsafe { UnregisterClassW(NAME, Some(module)) }.unwrap();
}
