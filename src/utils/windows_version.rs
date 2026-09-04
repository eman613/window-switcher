use windows::{
    Wdk::System::SystemServices::RtlGetVersion, Win32::System::SystemInformation::OSVERSIONINFOW,
};

pub fn os_version_info() -> Option<OSVERSIONINFOW> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as _,
        ..Default::default()
    };

    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() {
        Some(info)
    } else {
        None
    }
}

pub fn is_win11() -> bool {
    if let Some(info) = os_version_info() {
        info.dwBuildNumber >= 22000
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_query_uses_the_matching_buffer_size() {
        let info = os_version_info().expect("RtlGetVersion should succeed on Windows");

        assert_eq!(
            info.dwOSVersionInfoSize as usize,
            std::mem::size_of::<OSVERSIONINFOW>()
        );
    }
}
