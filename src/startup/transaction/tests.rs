use super::*;

const EXE: &str = "D:\\A & B\\window-switcher.exe";
const SID: &str = "S-1-5-21-123";

#[derive(Default)]
struct MemoryBackend {
    snapshot: Snapshot,
    writes: Vec<Entry>,
    fail_write: Option<usize>,
    deny_reads: bool,
    interfere: bool,
}

impl Backend for MemoryBackend {
    fn read(&mut self, entry: Entry) -> Result<Option<String>> {
        if self.deny_reads {
            bail!("fixture service unavailable");
        }
        Ok(self.snapshot.get(entry).map(str::to_owned))
    }
    fn exchange(
        &mut self,
        entry: Entry,
        expected: Option<&str>,
        replacement: Option<&str>,
    ) -> Result<Option<String>> {
        self.writes.push(entry);
        if self.fail_write == Some(self.writes.len()) {
            if self.interfere {
                self.snapshot.run = Some("\"D:\\external-change.exe\"".into());
            }
            bail!("fixture access denied");
        }
        if self.snapshot.get(entry) != expected {
            bail!("fixture concurrent change");
        }
        let value = replacement.map(str::to_owned);
        match entry {
            Entry::Run => self.snapshot.run = value.clone(),
            Entry::Task => self.snapshot.task = value.clone(),
        }
        Ok(value)
    }
}

fn task(highest: bool) -> String {
    TaskDefinition::create(
        EXE,
        SID,
        TaskPolicy {
            highest,
            allow_battery: true,
            stop_on_battery: true,
            enabled: true,
        },
    )
}

#[test]
fn auto_inherits_all_run_task_quadrants_without_changing_system_state() {
    for run in [false, true] {
        for scheduled in [false, true] {
            let mut backend = MemoryBackend {
                snapshot: Snapshot {
                    run: run.then(|| EXE.into()),
                    task: scheduled.then(|| task(true)),
                },
                ..Default::default()
            };
            assert_eq!(
                reconcile(&mut backend, &Config::default(), EXE, SID, false).unwrap(),
                run || scheduled
            );
            assert!(backend.writes.is_empty());
        }
    }
}

#[test]
fn creates_and_verifies_target_before_removing_old_run_entry() {
    let original_run = format!("\"{EXE}\"");
    let mut backend = MemoryBackend {
        snapshot: Snapshot {
            run: Some(original_run),
            task: None,
        },
        ..Default::default()
    };
    let configuration = Config {
        startup_enabled: StartupEnabled::Yes,
        startup_run_level: RunLevel::Highest,
        startup_battery_policy: BatteryPolicy::Allow,
        ..Default::default()
    };
    assert!(reconcile(&mut backend, &configuration, EXE, SID, true).unwrap());
    assert_eq!(backend.writes, [Entry::Task, Entry::Run]);
    assert!(backend.snapshot.run.is_none());
    let installed = TaskDefinition::parse(backend.snapshot.task.as_ref().unwrap()).unwrap();
    assert!(installed.policy.highest);
    assert!(installed.policy.allow_battery);
    assert!(!installed.policy.stop_on_battery);
}

#[test]
fn creation_and_old_entry_removal_failures_restore_previous_working_state() {
    for failure in [1, 2] {
        let original = format!("\"{EXE}\"");
        let mut backend = MemoryBackend {
            snapshot: Snapshot {
                run: Some(original.clone()),
                task: None,
            },
            fail_write: Some(failure),
            ..Default::default()
        };
        let configuration = Config {
            startup_enabled: StartupEnabled::Yes,
            startup_run_level: RunLevel::Highest,
            ..Default::default()
        };
        assert!(reconcile(&mut backend, &configuration, EXE, SID, true).is_err());
        assert_eq!(backend.snapshot.run.as_deref(), Some(original.as_str()));
        assert!(backend.snapshot.task.is_none());
    }
}

#[test]
fn disable_failure_restores_the_entry_removed_first() {
    let original = format!("\"{EXE}\"");
    let mut backend = MemoryBackend {
        snapshot: Snapshot {
            run: Some(original.clone()),
            task: Some(task(true)),
        },
        fail_write: Some(2),
        ..Default::default()
    };
    let configuration = Config {
        startup_enabled: StartupEnabled::No,
        ..Default::default()
    };
    assert!(reconcile(&mut backend, &configuration, EXE, SID, true).is_err());
    assert_eq!(backend.snapshot.run.as_deref(), Some(original.as_str()));
    assert!(backend.snapshot.task.is_some());
}

#[test]
fn failed_rollback_keeps_new_target_and_never_overwrites_external_changes() {
    let mut backend = MemoryBackend {
        snapshot: Snapshot {
            run: Some(EXE.into()),
            task: None,
        },
        fail_write: Some(2),
        interfere: true,
        ..Default::default()
    };
    let configuration = Config {
        startup_enabled: StartupEnabled::Yes,
        startup_run_level: RunLevel::Highest,
        ..Default::default()
    };
    assert!(reconcile(&mut backend, &configuration, EXE, SID, true).is_err());
    assert_eq!(
        backend.snapshot.run.as_deref(),
        Some("\"D:\\external-change.exe\"")
    );
    assert!(backend.snapshot.task.is_some());
}

#[test]
fn unknown_service_foreign_entries_and_insufficient_privilege_never_disable_old_startup() {
    let configuration = Config {
        startup_enabled: StartupEnabled::Yes,
        startup_run_level: RunLevel::Highest,
        ..Default::default()
    };
    for mut backend in [
        MemoryBackend {
            snapshot: Snapshot {
                run: Some(EXE.into()),
                task: None,
            },
            ..Default::default()
        },
        MemoryBackend {
            deny_reads: true,
            ..Default::default()
        },
        MemoryBackend {
            snapshot: Snapshot {
                run: Some("foreign.exe".into()),
                task: None,
            },
            ..Default::default()
        },
    ] {
        let before = backend.snapshot.run.clone();
        assert!(reconcile(&mut backend, &configuration, EXE, SID, false).is_err());
        assert!(backend.writes.is_empty());
        assert_eq!(backend.snapshot.run, before);
    }
}

#[test]
fn inherit_battery_keeps_legacy_policy_and_existing_highest_task_without_elevation() {
    let xml = task(true);
    let mut backend = MemoryBackend {
        snapshot: Snapshot {
            run: None,
            task: Some(xml.clone()),
        },
        ..Default::default()
    };
    let configuration = Config {
        startup_enabled: StartupEnabled::Yes,
        ..Default::default()
    };
    assert!(reconcile(&mut backend, &configuration, EXE, SID, false).unwrap());
    assert!(backend.writes.is_empty());
    assert_eq!(backend.snapshot.task.as_deref(), Some(xml.as_str()));
    let configuration = Config {
        startup_run_level: RunLevel::Standard,
        startup_battery_policy: BatteryPolicy::Stop,
        ..configuration
    };
    assert!(reconcile(&mut backend, &configuration, EXE, SID, true).unwrap());
    let updated = TaskDefinition::parse(backend.snapshot.task.as_ref().unwrap()).unwrap();
    assert!(!updated.policy.highest);
    assert!(!updated.policy.allow_battery);
    assert!(updated.policy.stop_on_battery);
}
