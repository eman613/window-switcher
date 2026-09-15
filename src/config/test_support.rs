use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TestDirectory(pub(crate) PathBuf);

impl TestDirectory {
    pub(crate) fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "window-switcher-stage-b-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create owned test directory: {error}"),
            }
        }
    }
    pub(crate) fn ini(&self) -> PathBuf {
        self.0.join("window-switcher.ini")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if !std::thread::panicking() {
                panic!("owned test directory cleanup: {error}");
            }
        }
    }
}
