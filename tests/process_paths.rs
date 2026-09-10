use std::{
    env,
    ffi::OsString,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use window_switcher::utils::{get_exe_folder, get_exe_path, get_module_path};

const PROBE_ENVIRONMENT: &str = "WINDOW_SWITCHER_LONG_PATH_PROBE";
const PROBE_TEST: &str = "complete_paths_from_long_unicode_directory";

struct LongPathProbe {
    directory: PathBuf,
    child: Option<Child>,
}

impl LongPathProbe {
    fn start() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "window-switcher-path-probe-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let mut probe = Self {
            directory,
            child: None,
        };
        let mut executable_directory = probe.directory.clone();
        while executable_directory.as_os_str().encode_wide().count() < 340 {
            executable_directory =
                executable_directory.join(format!("中文-\u{1D11E}-{}", "路径".repeat(24)));
        }
        fs::create_dir_all(&executable_directory).unwrap();
        let executable = executable_directory.join("process-path-probe.exe");
        fs::copy(env::current_exe().unwrap(), &executable).unwrap();
        let executable = fs::canonicalize(executable).unwrap();
        probe.child = Some(
            Command::new(&executable)
                .args(["--exact", PROBE_TEST, "--nocapture", "--test-threads=1"])
                .env(PROBE_ENVIRONMENT, &executable)
                .env("RUST_BACKTRACE", "0")
                .current_dir(&probe.directory)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("Failed to start the long-path test process"),
        );
        probe
    }

    fn wait(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let child = self.child.as_mut().unwrap();
        loop {
            if let Some(status) = child.try_wait().expect("Failed to observe the path probe") {
                assert!(status.success(), "Long-path test process failed: {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Long-path test process timed out"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for LongPathProbe {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("Failed to clean the long-path test directory: {error}");
        }
    }
}

#[test]
fn complete_paths_from_long_unicode_directory() {
    let Some(expected_path) = env::var_os(PROBE_ENVIRONMENT) else {
        LongPathProbe::start().wait();
        return;
    };

    simple_logging::log_to_stderr(log::LevelFilter::Debug);
    let expected_path = PathBuf::from(expected_path);
    let expected_length = expected_path.as_os_str().encode_wide().count();
    assert!(expected_length > 260);
    let executable_path = get_exe_path().expect("Failed to query the full executable path");
    let process_path = get_module_path(std::process::id());
    println!(
        "long path probe expected_utf16={expected_length} executable_utf16={} process_path_present={}",
        executable_path.len(),
        process_path.is_some()
    );
    let process_path = process_path.expect("Long process image path must not be dropped");
    assert!(executable_path.len() > 260);
    assert!(process_path.encode_utf16().count() > 260);
    let expected_path = fs::canonicalize(expected_path).unwrap();
    assert_eq!(
        fs::canonicalize(PathBuf::from(OsString::from_wide(&executable_path))).unwrap(),
        expected_path
    );
    assert_eq!(fs::canonicalize(process_path).unwrap(), expected_path);
    assert_eq!(
        fs::canonicalize(get_exe_folder().unwrap()).unwrap(),
        expected_path.parent().unwrap()
    );
}

#[test]
fn current_process_paths_agree() {
    let expected = fs::canonicalize(env::current_exe().unwrap()).unwrap();
    let executable = PathBuf::from(OsString::from_wide(&get_exe_path().unwrap()));
    let process_path = get_module_path(std::process::id()).unwrap();
    assert_eq!(fs::canonicalize(executable).unwrap(), expected);
    assert_eq!(fs::canonicalize(process_path).unwrap(), expected);
}

#[test]
fn invalid_process_id_is_not_a_path() {
    assert!(get_module_path(0).is_none());
}
