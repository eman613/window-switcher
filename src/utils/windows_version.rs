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
        warn!("platform stage=version ntstatus={:#x}", status.0);
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
    fn version_query_matches_the_real_structure_and_preserves_canary() {
        #[repr(C)]
        struct GuardedVersion {
            info: OSVERSIONINFOW,
            canary: [u8; 16],
        }
        let mut guarded = GuardedVersion {
            info: OSVERSIONINFOW {
                dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
                ..Default::default()
            },
            canary: [0xa5; 16],
        };
        let status = unsafe { RtlGetVersion(&mut guarded.info) };
        assert!(status.is_ok());
        assert_eq!(guarded.canary, [0xa5; 16]);
        let actual = os_version_info().unwrap();
        assert_eq!(
            actual.dwOSVersionInfoSize as usize,
            std::mem::size_of::<OSVERSIONINFOW>()
        );
        assert_eq!(actual.dwBuildNumber, guarded.info.dwBuildNumber);
    }
}
