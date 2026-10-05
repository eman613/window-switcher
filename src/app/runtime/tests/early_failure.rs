use super::*;
use windows::{
    core::BOOL,
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        UI::WindowsAndMessaging::{EnumThreadWindows, GetClassNameW},
    },
};

fn panel_count_on_current_thread() -> usize {
    unsafe extern "system" fn count(hwnd: HWND, data: LPARAM) -> BOOL {
        let mut name = [0u16; 128];
        let length = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
        if &name[..length] == unsafe { crate::app::NAME.as_wide() } {
            unsafe { *(data.0 as *mut usize) += 1 };
        }
        BOOL(1)
    }
    let mut total = 0usize;
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(count),
            LPARAM((&mut total as *mut usize) as isize),
        )
    }
    .unwrap();
    total
}

#[test]
fn runtime_early_failures_release_instance_window_and_apartment() {
    std::thread::spawn(|| {
        let name = format!("WindowSwitcherEarlyFailureTest-{}", std::process::id());
        // A deliberately invalid layout stops the real startup after creating
        // its HWND, before hooks, workers, startup settings or file writes.
        let loaded = LoadedConfig {
            config: crate::config::Config {
                panel_width: 1,
                search_enable: true,
                ..Default::default()
            },
            path: Default::default(),
            migrated: false,
            contents: Vec::new(),
        };
        assert_eq!(panel_count_on_current_thread(), 0);
        let mta = ComApartment::mta().unwrap();
        let instance = SingleInstance::create(&name).unwrap();
        assert!(instance.is_single());
        let error = run(&loaded, instance, None, Default::default()).unwrap_err();
        assert!(error.to_string().contains("shell stage=com-initialize"));
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            RPC_E_CHANGED_MODE
        );
        assert_eq!(panel_count_on_current_thread(), 0);
        // The failed STA request must not release the caller's MTA reference.
        assert!(ComApartment::sta().is_err());
        drop(mta);

        let instance = SingleInstance::create(&name).unwrap();
        assert!(
            instance.is_single(),
            "COM failure retained the instance mutex"
        );
        let error = run(&loaded, instance, None, Default::default()).unwrap_err();
        assert!(error.to_string().contains("panel_width"));
        assert_eq!(panel_count_on_current_thread(), 0);
        // Successful STA initialization must be balanced on the later failure.
        let mta = ComApartment::mta().unwrap();
        let instance = SingleInstance::create(&name).unwrap();
        assert!(
            instance.is_single(),
            "painter failure retained the instance mutex"
        );
        drop(instance);
        drop(mta);
        let _sta = ComApartment::sta().unwrap();
    })
    .join()
    .unwrap();
}
