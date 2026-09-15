use std::{fs, time::Instant};

use super::*;
use crate::config::{reload::load_snapshot, test_support::TestDirectory, StartupEnabled};
use windows::Win32::Foundation::HWND;

#[test]
fn tray_save_changes_desired_ini_and_keeps_the_effective_checkmark_until_restart() {
    let directory = TestDirectory::new();
    let text = "; custom comment\nauto_restart=no\n[switch-apps]\nbadge_max=300\n[startup]\nenabled=auto\n";
    fs::write(directory.ini(), text).unwrap();
    let loaded = load_snapshot(text.as_bytes(), &directory.ini()).unwrap();
    let mut startup = Startup {
        state: StartupState::Ready(true),
        configuration: Some(loaded.config),
        path: directory.ini(),
        target: Some(Arc::new(WindowTarget::new(HWND::default()))),
        events: None,
        canceled: Arc::new(AtomicBool::new(false)),
    };
    startup.toggle().unwrap();
    assert!(startup.busy());
    assert!(startup.toggle().is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = startup.poll() {
            assert!(matches!(result, StartupUpdate::Saved(Ok(()))));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(startup.state, StartupState::Saved(true));
    assert!(!startup.busy());
    assert!(startup.toggle().is_err());
    let saved = fs::read(directory.ini()).unwrap();
    let configured = load_snapshot(&saved, &directory.ini()).unwrap().config;
    assert_eq!(configured.startup_enabled, StartupEnabled::No);
    assert_eq!(configured.switch_apps_badge_max, 300);
    assert!(!configured.auto_restart);
    assert!(String::from_utf8(saved)
        .unwrap()
        .starts_with("; custom comment\n"));
}

#[test]
fn worker_exit_is_unknown_instead_of_disabled_and_canceled_ui_cannot_write() {
    let (tx, rx) = mpsc::sync_channel(1);
    let mut startup = Startup {
        events: Some(rx),
        state: StartupState::Pending,
        canceled: Arc::new(AtomicBool::new(false)),
        configuration: None,
        path: PathBuf::new(),
        target: None,
    };
    assert!(startup.poll().is_none());
    drop(tx);
    assert!(matches!(
        startup.poll(),
        Some(StartupUpdate::Inspected(Err(_)))
    ));
    assert_eq!(startup.state, StartupState::Failed);
    assert!(!startup.busy());
    startup.state = StartupState::Ready(false);
    startup.cancel();
    assert!(startup.toggle().is_err());
}
