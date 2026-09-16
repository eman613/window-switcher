use super::*;
use crate::{
    process_metadata::{ProcessIdentity, ProcessMetadata},
    utils::window_identity::WindowIdentity,
    window_snapshot::WindowRecord,
};

pub(crate) fn record(group: &str, window: usize, pid: u32, created: u64) -> WindowRecord {
    let mut identity = WindowIdentity::fixture(window);
    identity.process = ProcessIdentity { pid, created };
    WindowRecord {
        identity,
        application: AppIdentity::plain(Arc::from(group)),
        process: ProcessMetadata {
            identity: identity.process,
            path: Arc::from("fixture.exe"),
            executable: Arc::from("fixture.exe"),
            elevated: Some(false),
        },
        title: format!("window {window}"),
        minimized: false,
    }
}

fn snapshot() -> WindowSnapshot {
    WindowSnapshot {
        revision: 9,
        groups: [(
            Arc::from("fixture.exe"),
            vec![
                record("fixture.exe", 1, 10, 1),
                record("fixture.exe", 2, 10, 1),
                record("fixture.exe", 3, 20, 1),
                record("fixture.exe", 4, 10, 2),
            ],
        )]
        .into_iter()
        .collect(),
    }
}

#[test]
fn process_groups_use_creation_time_but_keep_the_original_icon_source() {
    let config = Config {
        switch_apps_grouping: Grouping::Process,
        ..Default::default()
    };
    let mut resolver = GroupResolver::new(&config, Path::new("."));
    let result = resolver
        .regroup(snapshot(), SwitchKind::Apps, || true)
        .unwrap()
        .unwrap();
    assert_eq!(result.revision, 9);
    assert_eq!(
        result.groups.values().map(Vec::len).collect::<Vec<_>>(),
        [2, 1, 1]
    );
    for record in result.groups.values().flatten() {
        assert_eq!(&*record.application.icon_key, "fixture.exe");
        assert!(record.application.name("Fixture").contains("PID"));
    }
    let windows = resolver
        .regroup(snapshot(), SwitchKind::Windows, || true)
        .unwrap()
        .unwrap();
    assert_eq!(windows.groups.len(), 1);
    assert!(resolver
        .regroup(snapshot(), SwitchKind::Search, || false)
        .unwrap()
        .is_none());
}

#[test]
fn app_id_remains_compatible_and_profile_falls_back_without_browser_metadata() {
    for policy in [Grouping::AppId, Grouping::Profile] {
        let config = Config {
            switch_apps_grouping: policy,
            ..Default::default()
        };
        let mut resolver = GroupResolver::new(&config, Path::new("."));
        let result = resolver
            .regroup(snapshot(), SwitchKind::Apps, || true)
            .unwrap()
            .unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups.values().next().unwrap().len(), 4);
        assert_eq!(
            &*result.groups.values().next().unwrap()[0]
                .application
                .name("Fixture"),
            "Fixture"
        );
    }
}
