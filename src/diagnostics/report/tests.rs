use super::*;
#[test]
fn report_does_not_export_private_configuration_values() {
    let mut config = Config::default();
    let secret = "PRIVATE-SENTINEL-用户文档";
    config.switch_apps_exclude_titles.insert(secret.into());
    config
        .switch_apps_override_icons
        .insert(secret.into(), format!("C:\\{secret}\\icon.ico"));
    config.ui_font_family = secret.into();
    config.chrome_user_data_dir = Some(PathBuf::from(secret));
    let report = ReportSnapshot::capture(&config, false, true, StartupState::Failed, None, None)
        .render()
        .unwrap();
    assert!(!report.contains(secret));
    assert!(report.contains("startup_effective = query-failed"));
    assert!(report.contains("chrome_custom_root_configured = true"));
    assert!(report.contains("panel_dpi = not-collected"));
}
#[test]
fn export_does_not_overwrite_and_cancellation_leaves_no_partial_file() {
    let directory = std::env::temp_dir().join(format!(
        "report-test-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let first = write_report(&directory, "first", || true).unwrap();
    let second = write_report(&directory, "second", || true).unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
    let calls = std::cell::Cell::new(0);
    assert!(write_report(&directory, "cancel", || {
        calls.set(calls.get() + 1);
        calls.get() == 1
    })
    .is_err());
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
    assert!(write_report(&first, "invalid directory", || true).is_err());
    assert!(write_report(&directory, &"x".repeat(MAX_REPORT_BYTES + 1), || true).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}
