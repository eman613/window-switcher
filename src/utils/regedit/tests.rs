use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use windows::{
    core::w,
    Win32::System::Registry::{RegDeleteKeyW, REG_DWORD},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestKey {
    path: Vec<u16>,
    key: Option<RegKey>,
}

impl TestKey {
    fn new() -> Self {
        let path = crate::utils::to_wstring(&format!(
            "Software\\WindowSwitcherTest-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let key = RegKey::writable_hkcu(PCWSTR(path.as_ptr()), w!("fixture")).unwrap();
        Self {
            path,
            key: Some(key),
        }
    }

    fn key(&self) -> &RegKey {
        self.key.as_ref().unwrap()
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        self.key.take();
        let result = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(self.path.as_ptr())) }.ok();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
fn registry_strings_round_trip_missing_empty_long_and_unterminated_values() {
    let fixture = TestKey::new();
    let key = fixture.key();
    assert_eq!(key.get_string().unwrap(), None);
    key.compare_string(None, Some("")).unwrap();
    assert_eq!(key.get_string().unwrap().as_deref(), Some(""));
    let long = "路径 with spaces & <test> ".repeat(300);
    key.compare_string(Some(""), Some(&long)).unwrap();
    assert_eq!(key.get_string().unwrap().as_deref(), Some(long.as_str()));
    // RegGetValueW guarantees a terminator even for legacy REG_SZ without NUL.
    let raw: Vec<_> = "legacy".encode_utf16().flat_map(u16::to_le_bytes).collect();
    unsafe { RegSetValueExW(key.hkey, key.name(), None, REG_SZ, Some(&raw)) }
        .ok()
        .unwrap();
    assert_eq!(key.get_string().unwrap().as_deref(), Some("legacy"));
    key.compare_string(Some("legacy"), None).unwrap();
    assert_eq!(key.get_string().unwrap(), None);
}

#[test]
fn registry_wrong_types_embedded_nul_and_readonly_handles_are_rejected() {
    let fixture = TestKey::new();
    let key = fixture.key();
    unsafe {
        RegSetValueExW(
            key.hkey,
            key.name(),
            None,
            REG_DWORD,
            Some(&17u32.to_le_bytes()),
        )
    }
    .ok()
    .unwrap();
    assert_eq!(key.get_int().unwrap(), 17);
    assert!(key.get_string().is_err());
    let raw: Vec<_> = "bad\0tail\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    unsafe { RegSetValueExW(key.hkey, key.name(), None, REG_SZ, Some(&raw)) }
        .ok()
        .unwrap();
    assert!(key.get_string().is_err());
    unsafe { RegDeleteValueW(key.hkey, key.name()) }
        .ok()
        .unwrap();
    key.compare_string(None, Some("old")).unwrap();
    let readonly = RegKey::new_hkcu(PCWSTR(fixture.path.as_ptr()), w!("fixture")).unwrap();
    assert!(readonly.compare_string(Some("old"), Some("new")).is_err());
    assert_eq!(readonly.get_string().unwrap().as_deref(), Some("old"));
}

#[test]
fn registry_external_change_is_not_overwritten_by_a_stale_request() {
    let fixture = TestKey::new();
    let first = fixture.key();
    first.compare_string(None, Some("before")).unwrap();
    let external = RegKey::writable_hkcu(PCWSTR(fixture.path.as_ptr()), w!("fixture")).unwrap();
    external
        .compare_string(Some("before"), Some("external"))
        .unwrap();
    assert!(first.compare_string(Some("before"), None).is_err());
    assert_eq!(first.get_string().unwrap().as_deref(), Some("external"));
}
