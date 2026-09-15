use std::{fs, path::Path};

use super::*;
use crate::config::test_support::TestDirectory;
use windows::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

fn powershell(script: &str) -> Command {
    let mut command = Command::new("pwsh");
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    command
}

fn slow_command(path: &Path) -> Command {
    let mut command = powershell(
        "$ErrorActionPreference = 'Stop'; [IO.File]::WriteAllText($env:WINDOW_SWITCHER_TEST_PID, [string]$PID); Start-Sleep -Seconds 60",
    );
    command.env("WINDOW_SWITCHER_TEST_PID", path);
    command
}

fn assert_stopped(path: &Path) {
    let pid: u32 = fs::read_to_string(path).unwrap().parse().unwrap();
    match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => {
            let owned = crate::utils::HandleWrapper::new(handle);
            assert_eq!(
                unsafe { WaitForSingleObject(owned.get_handle(), 2000) },
                WAIT_OBJECT_0
            );
        }
        Err(error) => assert_eq!(error.code(), ERROR_INVALID_PARAMETER.to_hresult()),
    }
}

#[test]
fn command_captures_bounded_output_and_preserves_exit_status() {
    let output = run(
        &mut powershell(
            "$inputText = [Console]::In.ReadToEnd(); [Console]::Write($inputText); exit 7",
        ),
        b"private request".to_vec(),
        Duration::from_secs(10),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"private request");
}

#[test]
fn timeout_stops_and_reaps_only_the_owned_command() {
    let directory = TestDirectory::new();
    let pid = directory.0.join("pid.txt");
    let started = Instant::now();
    let error = run(
        &mut slow_command(&pid),
        Vec::new(),
        Duration::from_secs(3),
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert!(format!("{error:#}").contains("超时"));
    assert!(started.elapsed() < Duration::from_secs(7));
    assert_stopped(&pid);
}

#[test]
fn cancellation_interrupts_a_running_command_and_prevents_new_spawn() {
    let directory = TestDirectory::new();
    let pid = directory.0.join("pid.txt");
    let canceled = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(8);
            while !pid.exists() && Instant::now() < deadline {
                thread::sleep(CHECK_INTERVAL);
            }
            canceled.store(true, Ordering::Release);
        });
        let error = run(
            &mut slow_command(&pid),
            Vec::new(),
            Duration::from_secs(10),
            &canceled,
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("取消"));
    });
    assert_stopped(&pid);
    fs::remove_file(&pid).unwrap();
    assert!(run(
        &mut slow_command(&pid),
        Vec::new(),
        Duration::from_secs(10),
        &canceled
    )
    .is_err());
    assert!(!pid.exists());
}

#[test]
fn unbounded_child_output_is_rejected() {
    let result = run(
        &mut powershell("[Console]::Write(('x' * 3000000)); Start-Sleep -Seconds 60"),
        Vec::new(),
        Duration::from_secs(10),
        &AtomicBool::new(false),
    );
    assert!(format!("{:#}", result.err().unwrap()).contains("2 MiB"));
}
