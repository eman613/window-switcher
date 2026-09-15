use super::*;
use crate::{foreground::ForegroundWatcher, window_target::WindowTarget};
use windows::{
    core::w,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WM_APP},
    },
};

const WM_TEST_CALLBACKS: u32 = WM_APP + 0x71;
const PROBE_CALLBACKS: u64 = 2000;

// Test-only messages invoke production input matching on its owning input
// thread. They never use SendInput or forward synthetic keys to other apps.
pub(super) fn handle_probe(message: &MSG) -> bool {
    if message.message != WM_TEST_CALLBACKS {
        return false;
    }
    let mut durations = Vec::with_capacity(PROBE_CALLBACKS as usize);
    for index in 0..PROBE_CALLBACKS {
        let data = KBDLLHOOKSTRUCT {
            scanCode: 0x1e,
            flags: if index % 2 == 0 {
                Default::default()
            } else {
                LLKHF_UP
            },
            ..Default::default()
        };
        let started = Instant::now();
        // Match the exact decision path, without CallNextHookEx outside an OS
        // hook dispatch (another hook must never receive this private fixture).
        HOOK_CONTEXT.with(|slot| {
            assert!(!slot.borrow_mut().as_mut().unwrap().process(data));
        });
        durations.push(started.elapsed().as_nanos());
    }
    durations.sort_unstable();
    eprintln!(
        "stage=a input_probe samples={} p50_ns={} p95_ns={} p99_ns={}",
        durations.len(),
        durations[999],
        durations[1899],
        durations[1979]
    );
    true
}

struct MessageWindow(HWND);
impl MessageWindow {
    fn new() -> Self {
        Self(
            unsafe {
                CreateWindowExW(
                    Default::default(),
                    w!("STATIC"),
                    w!("input fixture"),
                    Default::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        )
    }
}
impl Drop for MessageWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) }.unwrap();
    }
}

#[test]
fn negative_code_unknown_message_null_and_unaligned_parameters_are_forwarded() {
    assert!(!can_read_keyboard_data(
        -1,
        WPARAM(WM_KEYDOWN as usize),
        LPARAM(1)
    ));
    assert!(!can_read_keyboard_data(0, WPARAM(0), LPARAM(8)));
    assert!(!can_read_keyboard_data(
        0,
        WPARAM(WM_KEYDOWN as usize),
        LPARAM(0)
    ));
    assert!(!can_read_keyboard_data(
        0,
        WPARAM(WM_KEYDOWN as usize),
        LPARAM(1)
    ));
}

#[test]
fn input_thread_is_ready_while_ui_is_not_pumping_and_shutdown_is_bounded() {
    let window = MessageWindow::new();
    let target = Arc::new(WindowTarget::new(window.0));
    let dispatch = Arc::new(InputDispatch::new(target.clone()));
    let foreground = ForegroundWatcher::init(&Default::default(), window.0).unwrap();
    // Empty bindings ensure the real installed hook cannot consume user keys.
    let started = Instant::now();
    let mut listener = KeyboardListener::init(dispatch.clone(), foreground.status(), &[]).unwrap();
    let ready_us = started.elapsed().as_micros();
    unsafe { PostThreadMessageW(listener.thread_id, WM_TEST_CALLBACKS, WPARAM(0), LPARAM(0)) }
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while dispatch.callback_statistics().0 < PROBE_CALLBACKS && Instant::now() < deadline {
        // Deliberately do not dispatch the window's UI queue.
        thread::sleep(Duration::from_millis(1));
    }
    let (count, maximum) = dispatch.callback_statistics();
    assert!(
        count >= PROBE_CALLBACKS,
        "the input thread depended on the UI queue"
    );
    target.close();
    assert!(!target.try_post(dispatch::WM_INPUT_READY));
    let worker = listener.thread.take().unwrap();
    drop(listener);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        worker.is_finished(),
        "the input worker did not stop within its test deadline"
    );
    worker.join().unwrap();
    dispatch.log_summary();
    eprintln!(
        "stage=a input_ready_us={ready_us} observed_callbacks={count} max_callback_us={maximum}"
    );
}

#[test]
fn cancellation_before_hook_install_never_reports_ready() {
    let window = MessageWindow::new();
    let target = Arc::new(WindowTarget::new(window.0));
    let dispatch = Arc::new(InputDispatch::new(target));
    let foreground = ForegroundWatcher::init(&Default::default(), window.0).unwrap();
    let (ready, result) = mpsc::sync_channel(1);
    assert!(run_input_thread(
        Vec::new(),
        dispatch,
        foreground.status(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(InputActivation::new(true)),
        &ready
    )
    .is_err());
    assert!(result.try_recv().is_err());
}

#[test]
#[ignore = "installs a process-global logger; run in an isolated test process"]
fn stage_a_synthetic_input_logs_are_anonymous() {
    use parking_lot::Mutex;
    use std::io::{self, Write};

    struct CapturedLog(Arc<Mutex<Vec<u8>>>);
    impl Write for CapturedLog {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let captured = Arc::new(Mutex::new(Vec::new()));
    simple_logging::log_to(CapturedLog(captured.clone()), log::LevelFilter::Trace);
    input_thread_is_ready_while_ui_is_not_pumping_and_shutdown_is_bounded();
    let _com = crate::utils::com::ComApartment::sta().unwrap();
    let groups = crate::utils::list_windows(false, false, false).unwrap();
    let bytes = captured.lock();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("input stage=summary"));
    assert!(text.contains("window stage=enumerated"));
    for forbidden in ["KBDLLHOOKSTRUCT", "scanCode", "vkCode", "Config {"] {
        assert!(
            !text.contains(forbidden),
            "raw input or configuration leaked"
        );
    }
    for (path, windows) in groups {
        if path.len() > 3 {
            assert!(!text.contains(&path), "a personal application path leaked");
        }
        for (hwnd, _) in windows {
            let title = crate::utils::get_window_title(hwnd);
            if title.len() > 3 {
                assert!(!text.contains(&title), "a window title leaked");
            }
        }
    }
    eprintln!(
        "stage=a privacy log_bytes={} raw_input_config_paths_titles=absent",
        bytes.len()
    );
}
