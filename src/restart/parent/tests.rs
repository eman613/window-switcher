use std::fs;

use super::*;
use crate::config::test_support::TestDirectory;
use windows::Win32::{
    Foundation::HWND,
    System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    },
};

// A real owned peer exercises pipe IO, early exit, cleanup and timeouts. It does
// not install hooks or touch the actual app mutex or user's startup entries.
const PEER: &str = r#"
$ErrorActionPreference = 'Stop'
[IO.File]::WriteAllText($env:WINDOW_SWITCHER_RESTART_PID, [string]$PID)
$reader = [IO.BinaryReader]::new([Console]::OpenStandardInput())
$writer = [Console]::OpenStandardOutput()
$header = $reader.ReadBytes(8)
$length = [BitConverter]::ToUInt32($header, 4)
$snapshot = $reader.ReadBytes($length)
if ($snapshot.Length -ne $length) { exit 20 }
if ($env:WINDOW_SWITCHER_RESTART_PHASE -eq 'startup') { exit 21 }
$writer.WriteByte(1); $writer.Flush()
if ($reader.ReadByte() -ne 2) { exit 22 }
if ($env:WINDOW_SWITCHER_RESTART_PHASE -eq 'activate') { exit 23 }
if ($env:WINDOW_SWITCHER_RESTART_PHASE -eq 'active-timeout') { Start-Sleep -Seconds 60 }
$writer.WriteByte(3); $writer.Flush()
if ($reader.ReadByte() -ne 4) { exit 24 }
if ($env:WINDOW_SWITCHER_RESTART_PHASE -eq 'commit') { exit 25 }
$writer.WriteByte(5); $writer.Flush()
Start-Sleep -Seconds 60
"#;

fn run_peer(phase: &str, cancel: bool, supersede: bool) {
    let directory = TestDirectory::new();
    fs::write(directory.ini(), b"trayicon=no\n").unwrap();
    let pid_file = directory.0.join("peer.pid");
    let mut command = Command::new("pwsh");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", PEER])
        .env("WINDOW_SWITCHER_RESTART_PID", &pid_file)
        .env("WINDOW_SWITCHER_RESTART_PHASE", phase);
    if phase == "spawn" {
        command = Command::new(directory.0.join("missing.exe"));
    }
    let candidate = ConfigCandidate {
        generation: 1,
        contents: b"trayicon=no\n".as_slice().into(),
    };
    let latest = Arc::new(AtomicU64::new(1));
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let decision = Arc::new(Decision::default());
    let (events_tx, events) = mpsc::sync_channel(4);
    let (commands, commands_rx) = mpsc::sync_channel(2);
    let worker = Worker {
        candidate: candidate.clone(),
        path: directory.ini(),
        latest: latest.clone(),
        target: target.clone(),
        decision: decision.clone(),
        deadline: Instant::now() + Duration::from_secs(4),
        events: events_tx,
        commands: commands_rx,
    };
    let controller = RestartController {
        candidate,
        events,
        commands,
        decision,
    };
    let thread = thread::spawn(move || worker.run_command(Ok(command)));
    let mut suspended = false;
    let mut ready = false;
    let mut accepted = false;
    loop {
        match controller
            .events
            .recv_timeout(Duration::from_secs(8))
            .unwrap()
        {
            ParentEvent::Suspend => {
                suspended = true;
                if cancel {
                    controller.cancel();
                } else if supersede {
                    latest.store(2, Ordering::Release);
                } else {
                    controller.suspended().unwrap();
                }
            }
            ParentEvent::Ready => {
                assert!(suspended);
                ready = true;
                controller.commit().unwrap();
            }
            ParentEvent::Done => {
                assert!(ready);
                accepted = controller.accept();
                assert!(accepted);
                // Reproduce closing the UI immediately after accepting the ACK.
                target.close();
                break;
            }
            ParentEvent::Failed { safe_to_resume, .. } => {
                assert!(safe_to_resume);
                break;
            }
        }
    }
    thread.join().unwrap();
    assert_eq!(accepted, phase == "success" && !cancel && !supersede);
    assert_eq!(suspended, !matches!(phase, "startup" | "spawn"));
    if pid_file.exists() {
        let pid = fs::read_to_string(pid_file).unwrap().parse().unwrap();
        if let Ok(handle) =
            unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) }
        {
            let handle = crate::utils::HandleWrapper::new(handle);
            let status = unsafe { WaitForSingleObject(handle.get_handle(), 0) };
            if accepted {
                assert_eq!(status, windows::Win32::Foundation::WAIT_TIMEOUT);
                unsafe { TerminateProcess(handle.get_handle(), 0) }.unwrap();
            }
            assert_eq!(
                unsafe { WaitForSingleObject(handle.get_handle(), 2000) },
                windows::Win32::Foundation::WAIT_OBJECT_0
            );
        } else {
            assert!(!accepted);
        }
    } else {
        assert_eq!(phase, "spawn");
    }
}

#[test]
fn owned_peer_failure_matrix_confirms_stop_before_old_instance_can_resume() {
    for phase in ["spawn", "startup", "activate", "commit", "active-timeout"] {
        run_peer(phase, false, false);
    }
}

#[test]
fn user_exit_and_new_generation_cancel_and_reap_a_prepared_peer() {
    run_peer("success", true, false);
    run_peer("success", false, true);
}

#[test]
fn accepted_child_survives_immediate_old_ui_close() {
    run_peer("success", false, false);
}
