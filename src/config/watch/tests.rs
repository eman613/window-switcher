use std::fs;

use super::*;
use crate::config::{reload::load_snapshot, test_support::TestDirectory};
use windows::Win32::Foundation::HWND;

fn next_candidate(watcher: &ConfigWatcher) -> ConfigCandidate {
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        match watcher.next_event() {
            Some(ConfigEvent::Candidate(candidate)) => return candidate,
            Some(ConfigEvent::Invalid(error)) => panic!("unexpected watcher failure: {error}"),
            None => {}
        }
        assert!(
            Instant::now() < until,
            "watcher failed to deliver a candidate"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn notify_and_poll_modes_deliver_exact_snapshots_even_when_postmessage_fails() {
    for mode in ["auto", "notify", "poll"] {
        let directory = TestDirectory::new();
        let initial = format!("trayicon=yes\nrestart_delay_ms=200\n[config]\nwatch_mode={mode}\npoll_interval_ms=100\nretry_delay_ms=200\nretry_limit=1\n");
        fs::write(directory.ini(), &initial).unwrap();
        let loaded = load_snapshot(initial.as_bytes(), &directory.ini()).unwrap();
        // No HWND: the same pending slot used by the UI timer must still work.
        let target = Arc::new(WindowTarget::new(HWND::default()));
        let watcher = ConfigWatcher::start(&loaded, target.clone()).unwrap();
        let next = initial.replace("trayicon=yes", "trayicon=no");
        fs::write(directory.ini(), &next).unwrap();
        let candidate = next_candidate(&watcher);
        assert_eq!(candidate.contents.as_ref(), next.as_bytes());
        assert_eq!(
            watcher.latest().load(Ordering::Acquire),
            candidate.generation
        );
        watcher.complete(candidate.generation, true);
        thread::sleep(Duration::from_millis(350));
        assert!(watcher.next_event().is_none());
        target.close();
        watcher.control.try_send(Control::Stop).unwrap();
        watcher
            .stopped
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }
}

#[test]
fn unchanged_candidate_retries_after_failure_then_stops_at_limit() {
    let directory = TestDirectory::new();
    let initial = "trayicon=yes\nrestart_delay_ms=200\n[config]\nwatch_mode=poll\npoll_interval_ms=100\nretry_delay_ms=200\nretry_limit=1\n";
    fs::write(directory.ini(), initial).unwrap();
    let loaded = load_snapshot(initial.as_bytes(), &directory.ini()).unwrap();
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let watcher = ConfigWatcher::start(&loaded, target.clone()).unwrap();
    fs::write(
        directory.ini(),
        initial.replace("trayicon=yes", "trayicon=no"),
    )
    .unwrap();
    let first = next_candidate(&watcher);
    watcher.complete(first.generation, false);
    let retry = next_candidate(&watcher);
    assert_eq!(first.generation, retry.generation);
    assert_eq!(first.contents, retry.contents);
    watcher.complete(retry.generation, false);
    thread::sleep(Duration::from_millis(700));
    assert!(watcher.next_event().is_none());
    target.close();
    watcher.control.try_send(Control::Stop).unwrap();
    watcher
        .stopped
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
}

#[test]
fn temporary_read_error_cannot_erase_an_undelivered_candidate() {
    let directory = TestDirectory::new();
    let initial =
        "trayicon=yes\nrestart_delay_ms=200\n[config]\nwatch_mode=poll\npoll_interval_ms=100\n";
    fs::write(directory.ini(), initial).unwrap();
    let loaded = load_snapshot(initial.as_bytes(), &directory.ini()).unwrap();
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let watcher = ConfigWatcher::start(&loaded, target.clone()).unwrap();
    let first = initial.replace("trayicon=yes", "trayicon=no");
    fs::write(directory.ini(), &first).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while watcher.pending.lock().candidate.is_none() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    fs::remove_file(directory.ini()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while watcher.pending.lock().invalid.is_none() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let old = next_candidate(&watcher);
    assert_eq!(old.contents.as_ref(), first.as_bytes());
    assert!(matches!(
        watcher.next_event(),
        Some(ConfigEvent::Invalid(_))
    ));
    watcher.complete(old.generation, false);
    let next = format!("{first}; second save\n");
    fs::write(directory.ini(), &next).unwrap();
    assert_eq!(next_candidate(&watcher).contents.as_ref(), next.as_bytes());
    target.close();
    watcher.control.try_send(Control::Stop).unwrap();
    watcher
        .stopped
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
}
