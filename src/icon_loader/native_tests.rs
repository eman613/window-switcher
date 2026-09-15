use super::*;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};
use windows::{
    core::w,
    Win32::{
        Foundation::{HINSTANCE, LRESULT},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateIcon, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
            PostMessageW, PostQuitMessage, RegisterClassW, UnregisterClassW, HWND_MESSAGE, MSG,
            WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
        },
    },
};

#[test]
fn native_icons_have_bounded_premultiplied_pixels_and_transparent_background() {
    let icon = fallback().unwrap();
    let image = rasterize(icon.0, 64).unwrap();
    assert!(image.data.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
    assert!(image.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    assert!(!topleft_only(&image));
    for p in image.data.as_chunks::<4>().0.iter() {
        assert!(p[..3].iter().all(|c| *c <= p[3]));
    }
    assert!(rasterize(HICON::default(), 64).is_err());
}

#[test]
fn monochrome_and_xor_masks_keep_transparent_pixels() {
    let mut mask = [255u8; 32];
    let mut color = [0u8; 32];
    for y in 4..12 {
        for x in 4..12 {
            let offset = y * 2 + x / 8;
            let bit = 1 << (7 - x % 8);
            mask[offset] &= !bit;
            color[offset] |= bit;
        }
    }
    let icon = OwnedIcon(
        unsafe { CreateIcon(None, 16, 16, 1, 1, mask.as_ptr(), color.as_ptr()) }.unwrap(),
    );
    let image = rasterize(icon.0, 16).unwrap();
    assert_eq!(&image.data[..4], &[0, 0, 0, 0]);
    assert_eq!(
        &image.data[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4],
        &[255, 255, 255, 255]
    );
}

static GETICON_CALLS: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn slow_icon_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_GETICON => {
            GETICON_CALLS.fetch_add(1, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(if wparam.0 == ICON_BIG as usize {
                20
            } else {
                400
            }));
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

struct SlowIconWindow {
    hwnd: HWND,
    thread: Option<JoinHandle<()>>,
}
impl SlowIconWindow {
    fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || unsafe {
            let module = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let name = w!("WindowSwitcherSlowIconTest");
            let class = WNDCLASSW {
                hInstance: module,
                lpszClassName: name,
                lpfnWndProc: Some(slow_icon_proc),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                name,
                name,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module),
                None,
            )
            .unwrap();
            sender.send(hwnd.0 as usize).unwrap();
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                DispatchMessageW(&message);
            }
            UnregisterClassW(name, Some(module)).unwrap();
        });
        Self {
            hwnd: HWND(receiver.recv_timeout(Duration::from_secs(3)).unwrap() as _),
            thread: Some(thread),
        }
    }
}
impl Drop for SlowIconWindow {
    fn drop(&mut self) {
        unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn slow_native_window_icon_queries_return_within_one_shared_timeout() {
    let window = SlowIconWindow::new();
    let started = Instant::now();
    assert!(window_icon(window.hwnd, Duration::from_millis(60)).is_none());
    let elapsed = started.elapsed();
    // Leave scheduling headroom on shared CI hosts, but never wait for the
    // deliberately blocked callback to complete on the querying thread.
    assert!(
        elapsed < Duration::from_millis(300),
        "native query blocked for {elapsed:?}"
    );
    assert!(GETICON_CALLS.load(Ordering::Relaxed) >= 1);
    eprintln!(
        "stage=c slow_icon shared_budget_ms=60 elapsed_us={} callbacks={}",
        elapsed.as_micros(),
        GETICON_CALLS.load(Ordering::Relaxed)
    );
    drop(window);
}
