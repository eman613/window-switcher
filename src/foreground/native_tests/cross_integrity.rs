use super::*;
use std::path::PathBuf;

const EXTERNAL_DIRECTORY: &str = "WINDOW_SWITCHER_CROSS_INTEGRITY_DIRECTORY";
const EXPECT_ELEVATED: &str = "WINDOW_SWITCHER_TEST_EXPECT_ELEVATED";

fn context() -> (PathBuf, bool) {
    let directory =
        PathBuf::from(std::env::var_os(EXTERNAL_DIRECTORY).expect("external test directory"));
    assert!(directory.is_dir());
    let elevated: bool = std::env::var(EXPECT_ELEVATED).unwrap().parse().unwrap();
    assert_eq!(crate::utils::is_running_as_admin().unwrap(), elevated);
    (directory, elevated)
}

struct ExternalPeer(PathBuf);

impl LifecyclePeer for ExternalPeer {
    fn signal(&mut self, command: &str) {
        fs::write(self.0.join(command), []).unwrap();
    }

    fn finish(&mut self) {
        self.signal("exit");
    }

    fn exited(&mut self) -> bool {
        self.0.join("peer-finished").exists()
    }
}

impl Drop for ExternalPeer {
    fn drop(&mut self) {
        // Release the external peer even if the watcher assertion unwinds.
        if let Err(error) = fs::write(self.0.join("exit"), []) {
            eprintln!("cross-integrity peer cleanup signal failed: {error}");
        }
    }
}

#[test]
#[ignore = "requires an externally coordinated peer with the opposite elevation"]
fn watcher() {
    let (directory, elevated) = context();
    let lifetimes = Arc::new(WindowLifetimes::default());
    let watcher =
        ForegroundWatcher::init(&Config::default(), HWND::default(), lifetimes.clone()).unwrap();
    let mut peer = ExternalPeer(directory.clone());
    fs::write(directory.join("watcher-ready"), []).unwrap();
    // Allow the launcher to complete the separately authorized elevation step.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !directory.join("first").exists() {
        pump();
        assert!(Instant::now() < deadline, "external peer startup timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    verify_lifecycle(&directory, lifetimes, watcher, &mut peer, Some(!elevated));
}

fn command(directory: &Path, name: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if directory.join("exit").exists() {
            return false;
        }
        if directory.join(name).exists() {
            return true;
        }
        assert!(
            Instant::now() < deadline,
            "external peer command timeout: {name}"
        );
        pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "internal external peer; requires a cross-integrity test coordinator"]
fn window_peer() {
    let (directory, _) = context();
    let first = HiddenWindow::create();
    first.publish(&directory, "first");
    if !command(&directory, "destroy") {
        return;
    }
    drop(first);
    if !command(&directory, "recreate") {
        return;
    }
    let second = HiddenWindow::create();
    second.publish(&directory, "second");
    assert!(!command(&directory, "unused"));
    drop(second);
    fs::write(directory.join("peer-finished"), []).unwrap();
}
