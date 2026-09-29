use super::*;
use crate::{config::test_support::TestDirectory, utils::window_identity::WindowIdentity};
use std::{
    fs,
    io::{BufRead, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    core::w,
    Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, MSG, PM_REMOVE, WS_POPUP,
    },
};

const PEER_DIRECTORY: &str = "WINDOW_SWITCHER_LIFECYCLE_TEST_DIRECTORY";
const PEER_TEST: &str = "foreground::native_tests::lifecycle_window_peer";
const DEADLINE: Duration = Duration::from_secs(10);

mod cross_integrity;

trait LifecyclePeer {
    fn signal(&mut self, command: &str);
    fn finish(&mut self);
    fn exited(&mut self) -> bool;
}

struct HiddenWindow(HWND);
impl HiddenWindow {
    fn create() -> Self {
        Self(
            unsafe {
                CreateWindowExW(
                    Default::default(),
                    w!("STATIC"),
                    w!("lifecycle test window"),
                    WS_POPUP,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        )
    }

    fn publish(&self, directory: &Path, name: &str) {
        fs::write(directory.join(name), (self.0 .0 as usize).to_string()).unwrap();
    }
}
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) }.unwrap();
    }
}

struct Peer(Child);
impl LifecyclePeer for Peer {
    fn signal(&mut self, command: &str) {
        writeln!(self.0.stdin.as_mut().unwrap(), "{command}").unwrap();
    }

    fn finish(&mut self) {
        drop(self.0.stdin.take());
    }

    fn exited(&mut self) -> bool {
        self.0
            .try_wait()
            .unwrap()
            .is_some_and(|status| status.success())
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        // EOF releases every blocking peer phase, including after an assertion.
        drop(self.0.stdin.take());
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            if self.0.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        eprintln!("lifecycle test peer did not exit within its cleanup deadline");
    }
}

fn pump() {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe { DispatchMessageW(&message) };
    }
}

fn await_condition(mut condition: impl FnMut() -> bool, stage: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        pump();
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "lifecycle event timeout: {stage}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn await_window(
    directory: &Path,
    name: &str,
    lifetimes: &WindowLifetimes,
    prior: Option<(usize, crate::window_snapshot::lifetimes::LifetimeStamp)>,
) -> HWND {
    let mut window = HWND::default();
    await_condition(
        || {
            let Some(value) = fs::read_to_string(directory.join(name))
                .ok()
                .and_then(|text| text.parse::<usize>().ok())
            else {
                return false;
            };
            window = HWND(value as _);
            let before = prior
                .filter(|(previous, _)| *previous == value)
                .map(|(_, stamp)| stamp)
                .or_else(|| WindowLifetimes::default().stamp(value));
            lifetimes
                .stamp(value)
                .is_some_and(|stamp| Some(stamp) != before)
        },
        name,
    );
    window
}

#[test]
#[ignore = "requires a desktop WinEvent hook and an isolated native peer process"]
fn real_cross_process_events_retire_window_identities_and_stop_after_unhook() {
    let directory = TestDirectory::new();
    let lifetimes = Arc::new(WindowLifetimes::default());
    let watcher =
        ForegroundWatcher::init(&Config::default(), HWND::default(), lifetimes.clone()).unwrap();
    let mut peer = Peer(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", PEER_TEST, "--test-threads=1"])
            .env(PEER_DIRECTORY, &directory.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    verify_lifecycle(&directory.0, lifetimes, watcher, &mut peer, None);
}

fn verify_lifecycle(
    directory: &Path,
    lifetimes: Arc<WindowLifetimes>,
    watcher: ForegroundWatcher,
    peer: &mut impl LifecyclePeer,
    expected_peer_elevated: Option<bool>,
) {
    let first = await_window(directory, "first", &lifetimes, None);
    let old = WindowIdentity::capture(first, &lifetimes).unwrap();
    if let Some(expected) = expected_peer_elevated {
        assert_eq!(
            crate::utils::is_process_elevated(old.process.pid),
            Some(expected)
        );
    }
    assert!(old.is_current(&lifetimes));
    peer.signal("destroy");
    await_condition(|| !old.has_current_lifetime(&lifetimes), "destroy event");
    assert!(!old.is_current(&lifetimes));

    let destroyed_stamp = lifetimes.stamp(old.window).unwrap();
    peer.signal("recreate");
    let second = await_window(
        directory,
        "second",
        &lifetimes,
        Some((old.window, destroyed_stamp)),
    );
    let current = WindowIdentity::capture(second, &lifetimes).unwrap();
    assert_eq!(old.process, current.process);
    assert_ne!(old, current);
    assert!(!old.is_current(&lifetimes));
    assert!(current.is_current(&lifetimes));
    drop(watcher);
    let revision = lifetimes.revision();
    peer.finish();
    await_condition(|| peer.exited(), "peer exit");
    pump();
    assert_eq!(lifetimes.revision(), revision);
    assert!(!current.is_current(&lifetimes));
}

#[test]
#[ignore = "internal peer; invoked only by the cross-process lifecycle regression"]
fn lifecycle_window_peer() {
    let Some(directory) = std::env::var_os(PEER_DIRECTORY) else {
        return;
    };
    let directory = Path::new(&directory);
    let mut commands = std::io::stdin().lock().lines();
    let first = HiddenWindow::create();
    first.publish(directory, "first");
    if commands.next().transpose().unwrap().as_deref() != Some("destroy") {
        return;
    }
    drop(first);
    if commands.next().transpose().unwrap().as_deref() != Some("recreate") {
        return;
    }
    let second = HiddenWindow::create();
    second.publish(directory, "second");
    assert!(commands.next().is_none());
}
