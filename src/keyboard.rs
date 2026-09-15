use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
            VK_RSHIFT, VK_RWIN,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
            LLKHF_EXTENDED, LLKHF_UP, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
            WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use crate::{config::Hotkey, foreground::ForegroundStatus};

pub(crate) mod dispatch;
pub(crate) mod state;

use dispatch::InputDispatch;
use state::{InputMachine, KeyInput};

thread_local! {
    static HOOK_CONTEXT: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

struct HookContext {
    machine: InputMachine,
    dispatch: Arc<InputDispatch>,
    foreground: Arc<ForegroundStatus>,
    stop: Arc<AtomicBool>,
}

impl HookContext {
    fn process(&mut self, data: KBDLLHOOKSTRUCT) -> bool {
        if self.stop.load(Ordering::Acquire) || !self.dispatch.is_live() {
            return false;
        }
        let started = Instant::now();
        let key = KeyInput {
            scan: data.scanCode,
            extended: data.flags.0 & LLKHF_EXTENDED.0 != 0,
            down: data.flags.0 & LLKHF_UP.0 == 0,
        };
        let decision = self.machine.handle(
            key,
            self.foreground.allows_windows(),
            self.dispatch.acknowledged(),
            self.dispatch.revoked(),
        );
        let accepted = decision
            .event
            .is_none_or(|event| self.dispatch.submit(event));
        let consume = accepted && decision.consume;
        if accepted {
            self.machine.accepted(key, consume);
        } else {
            self.machine.rejected(key);
        }
        self.dispatch.observe_callback(started.elapsed());
        consume
    }
}

pub(crate) struct KeyboardListener {
    stop: Arc<AtomicBool>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardListener {
    pub(crate) fn init(
        dispatch: Arc<InputDispatch>,
        foreground: Arc<ForegroundStatus>,
        hotkeys: &[&Hotkey],
    ) -> Result<Self> {
        let hotkeys = hotkeys.iter().map(|key| (**key).clone()).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("keyboard-input".into())
            .spawn(move || {
                if let Err(err) =
                    run_input_thread(hotkeys, dispatch, foreground, thread_stop, &ready_tx)
                {
                    error!("input stage=thread-failure error={err:#}");
                    let _ = ready_tx.try_send(Err(err));
                }
                HOOK_CONTEXT.with(|slot| slot.borrow_mut().take());
            })
            .context("input stage=thread-create")?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(thread_id)) => Ok(Self {
                stop,
                thread_id,
                thread: Some(thread),
            }),
            result => {
                stop.store(true, Ordering::Release);
                match result {
                    Ok(Err(err)) => Err(err),
                    _ => bail!("input stage=ready timeout or worker exit"),
                }
            }
        }
    }
}

impl Drop for KeyboardListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Err(err) =
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        {
            debug!("input stage=stop-notify code={:#x}", err.code().0);
        }
        // Never wait indefinitely for hook or OS message delivery on the UI thread.
        if let Some(thread) = self.thread.take() {
            if thread.is_finished() && thread.join().is_err() {
                warn!("input stage=thread-exit panic");
            }
        }
    }
}

struct HookOwner(HHOOK);
impl Drop for HookOwner {
    fn drop(&mut self) {
        if let Err(err) = unsafe { UnhookWindowsHookEx(self.0) } {
            warn!("input stage=unhook code={:#x}", err.code().0);
        }
    }
}

fn run_input_thread(
    hotkeys: Vec<Hotkey>,
    dispatch: Arc<InputDispatch>,
    foreground: Arc<ForegroundStatus>,
    stop: Arc<AtomicBool>,
    ready: &SyncSender<Result<u32>>,
) -> Result<()> {
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    if stop.load(Ordering::Acquire) {
        bail!("input stage=initialize canceled");
    }
    let mut machine = InputMachine::new(hotkeys);
    for (vk, scan, extended) in [
        (VK_LMENU, 0x38, false),
        (VK_RMENU, 0x38, true),
        (VK_LCONTROL, 0x1d, false),
        (VK_RCONTROL, 0x1d, true),
        (VK_LWIN, 0x5b, true),
        (VK_RWIN, 0x5c, true),
        (VK_LSHIFT, 0x2a, false),
        (VK_RSHIFT, 0x36, false),
    ] {
        machine.seed_modifier(scan, extended, unsafe { GetAsyncKeyState(vk.0 as i32) } < 0);
    }
    HOOK_CONTEXT.with(|slot| {
        *slot.borrow_mut() = Some(HookContext {
            machine,
            dispatch,
            foreground,
            stop: stop.clone(),
        })
    });
    let module = unsafe { GetModuleHandleW(None) }.context("input stage=module")?;
    let _hook = HookOwner(
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(module.into()), 0) }
            .context("input stage=hook-install")?,
    );
    ready
        .try_send(Ok(unsafe { GetCurrentThreadId() }))
        .map_err(|_| anyhow::anyhow!("input stage=ready receiver gone"))?;
    while !stop.load(Ordering::Acquire) {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result == -1 {
            return Err(windows::core::Error::from_win32()).context("input stage=message-loop");
        }
        if result == 0 {
            break;
        }
        #[cfg(test)]
        if tests::handle_probe(&message) {
            continue;
        }
        unsafe {
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn can_read_keyboard_data(code: i32, message: WPARAM, parameter: LPARAM) -> bool {
    code == HC_ACTION as i32
        && matches!(
            message.0 as u32,
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
        )
        && parameter.0 != 0
        && (parameter.0 as usize).is_multiple_of(std::mem::align_of::<KBDLLHOOKSTRUCT>())
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Negative codes must be forwarded without even reading lparam.
    if code < 0 || !can_read_keyboard_data(code, wparam, lparam) {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let data = (lparam.0 as *const KBDLLHOOKSTRUCT).read();
    let consume = HOOK_CONTEXT.with(|slot| {
        slot.try_borrow_mut()
            .ok()
            .and_then(|mut slot| slot.as_mut().map(|context| context.process(data)))
            .unwrap_or(false)
    });
    if consume {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}

#[cfg(test)]
mod tests;
