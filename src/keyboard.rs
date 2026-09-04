use crate::{
    app::{
        WM_USER_KEYBOARD_QUEUE, WM_USER_SWITCH_APPS, WM_USER_SWITCH_APPS_CANCEL,
        WM_USER_SWITCH_APPS_DONE, WM_USER_SWITCH_WINDOWS, WM_USER_SWITCH_WINDOWS_DONE,
    },
    config::{Hotkey, SWITCH_APPS_HOTKEY_ID, SWITCH_WINDOWS_HOTKEY_ID},
    foreground::IS_FOREGROUND_IN_BLACKLIST,
};

use anyhow::{anyhow, Result};
use indexmap::IndexSet;
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        LazyLock,
    },
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{SCANCODE_LSHIFT, SCANCODE_RSHIFT},
        WindowsAndMessaging::{
            CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK,
            KBDLLHOOKSTRUCT, LLKHF_UP, WH_KEYBOARD_LL,
        },
    },
};

static KEYBOARD_STATE: LazyLock<Mutex<Vec<HotKeyState>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static IS_SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static IS_SWITCHING_APPS: AtomicBool = AtomicBool::new(false);
static PREVIOUS_KEYCODE: AtomicU32 = AtomicU32::new(0);
static KEYBOARD_MESSAGES: LazyLock<Mutex<VecDeque<KeyboardMessage>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(KEYBOARD_QUEUE_CAPACITY)));
static KEYBOARD_WAKE_PENDING: AtomicBool = AtomicBool::new(false);

const KEYBOARD_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyboardMessage {
    pub(crate) msg: u32,
    pub(crate) wparam: WPARAM,
    pub(crate) lparam: LPARAM,
}

#[derive(Debug)]
pub struct KeyboardListener {
    hook: HHOOK,
}

impl KeyboardListener {
    pub fn init(hwnd: HWND, hotkeys: &[&Hotkey]) -> Result<Self> {
        WINDOW.store(hwnd.0 as isize, Ordering::Release);
        IS_SHIFT_PRESSED.store(false, Ordering::Release);
        IS_SWITCHING_APPS.store(false, Ordering::Release);
        PREVIOUS_KEYCODE.store(0, Ordering::Release);
        clear_keyboard_messages();

        let keyboard_state = hotkeys
            .iter()
            .map(|hotkey| HotKeyState {
                hotkey: (*hotkey).clone(),
                is_modifier_pressed: false,
            })
            .collect();
        *KEYBOARD_STATE.lock() = keyboard_state;

        let hook = unsafe {
            let hinstance = { GetModuleHandleW(None) }
                .map_err(|err| anyhow!("Failed to get module handle, {err}"))?;
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_proc),
                Some(hinstance.into()),
                0,
            )
        }
        .map_err(|err| anyhow!("Failed to set windows hook, {err}"))?;
        info!("keyboard listener start");

        Ok(Self { hook })
    }
}

impl Drop for KeyboardListener {
    fn drop(&mut self) {
        debug!("keyboard listener destroyed");
        if !self.hook.is_invalid() {
            let _ = unsafe { UnhookWindowsHookEx(self.hook) };
        }
        WINDOW.store(0, Ordering::Release);
        clear_keyboard_messages();
    }
}

#[derive(Debug)]
struct HotKeyState {
    hotkey: Hotkey,
    is_modifier_pressed: bool,
}

fn queue_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
    let Some(mut queue) = KEYBOARD_MESSAGES.try_lock() else {
        debug!("keyboard message queue lock unavailable; dropping message {msg}");
        return false;
    };

    if let Some(last) = queue.back() {
        if last.msg == msg && last.wparam == wparam && last.lparam == lparam {
            return true;
        }
    }

    if queue.len() >= KEYBOARD_QUEUE_CAPACITY {
        if is_completion_message(msg) {
            if let Some(index) = queue
                .iter()
                .position(|message| !is_completion_message(message.msg))
            {
                queue.remove(index);
            } else {
                debug!("keyboard message queue full; dropping completion {msg}");
                return false;
            }
        } else {
            debug!("keyboard message queue full; dropping message {msg}");
            return false;
        }
    }

    queue.push_back(KeyboardMessage {
        msg,
        wparam,
        lparam,
    });
    drop(queue);

    if !KEYBOARD_WAKE_PENDING.swap(true, Ordering::AcqRel) && !post_keyboard_wake() {
        drop_queued_messages();
        return false;
    }
    true
}

fn post_keyboard_wake() -> bool {
    let raw_hwnd = WINDOW.load(Ordering::Acquire);
    if raw_hwnd == 0 {
        KEYBOARD_WAKE_PENDING.store(false, Ordering::Release);
        return false;
    }

    let hwnd = HWND(raw_hwnd as _);
    if unsafe { PostMessageW(Some(hwnd), WM_USER_KEYBOARD_QUEUE, WPARAM(0), LPARAM(0)) }.is_err() {
        KEYBOARD_WAKE_PENDING.store(false, Ordering::Release);
        debug!("failed to post keyboard queue wake message");
        return false;
    }
    true
}

fn is_completion_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_USER_SWITCH_APPS_DONE | WM_USER_SWITCH_APPS_CANCEL | WM_USER_SWITCH_WINDOWS_DONE
    )
}

pub(crate) fn drain_keyboard_messages() -> Vec<KeyboardMessage> {
    let messages = {
        let mut queue = KEYBOARD_MESSAGES.lock();
        queue.drain(..).collect::<Vec<_>>()
    };

    KEYBOARD_WAKE_PENDING.store(false, Ordering::Release);
    let has_pending = !KEYBOARD_MESSAGES.lock().is_empty();
    if has_pending && !KEYBOARD_WAKE_PENDING.swap(true, Ordering::AcqRel) && !post_keyboard_wake() {
        drop_queued_messages();
    }
    messages
}

fn clear_keyboard_messages() {
    KEYBOARD_MESSAGES.lock().clear();
    KEYBOARD_WAKE_PENDING.store(false, Ordering::Release);
}

fn drop_queued_messages() {
    if let Some(mut queue) = KEYBOARD_MESSAGES.try_lock() {
        queue.clear();
    }
    KEYBOARD_WAKE_PENDING.store(false, Ordering::Release);
}

unsafe extern "system" fn keyboard_proc(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code < 0 || l_param.0 == 0 {
        return CallNextHookEx(None, code, w_param, l_param);
    }

    let Some(kbd_data) = (l_param.0 as *const KBDLLHOOKSTRUCT).as_ref() else {
        return CallNextHookEx(None, code, w_param, l_param);
    };
    debug!("keyboard {kbd_data:?}");
    let mut is_modifier = false;
    let scan_code = kbd_data.scanCode;
    let is_key_pressed = || kbd_data.flags.0 & LLKHF_UP.0 == 0;
    if [SCANCODE_LSHIFT, SCANCODE_RSHIFT].contains(&scan_code) {
        IS_SHIFT_PRESSED.store(is_key_pressed(), Ordering::Release);
    }
    let Some(mut keyboard_state) = KEYBOARD_STATE.try_lock() else {
        return CallNextHookEx(None, code, w_param, l_param);
    };
    let mut send_done_hotkeys: IndexSet<u32> = IndexSet::new();
    let mut send_action_message: Option<(u32, isize, bool)> = None;

    for state in keyboard_state.iter_mut() {
        if state.hotkey.modifier.contains(&scan_code) {
            is_modifier = true;
            if is_key_pressed() {
                state.is_modifier_pressed = true;
            } else {
                state.is_modifier_pressed = false;
                if PREVIOUS_KEYCODE.load(Ordering::Acquire) == state.hotkey.code {
                    send_done_hotkeys.insert(state.hotkey.id);
                }
            }
        }
    }
    if !is_modifier {
        for state in keyboard_state.iter_mut() {
            if is_key_pressed() && state.is_modifier_pressed {
                let id = state.hotkey.id;
                if scan_code == state.hotkey.code {
                    let reverse = if IS_SHIFT_PRESSED.load(Ordering::Acquire) {
                        1
                    } else {
                        0
                    };
                    if id == SWITCH_APPS_HOTKEY_ID
                        || (id == SWITCH_WINDOWS_HOTKEY_ID
                            && !IS_FOREGROUND_IN_BLACKLIST.load(Ordering::Acquire))
                    {
                        send_action_message = Some((id, reverse, false));
                        break;
                    };
                } else if id == SWITCH_APPS_HOTKEY_ID {
                    if scan_code == 0x01 {
                        // escape key
                        send_action_message = Some((id, 0, true));
                        break;
                    } else if [0x48, 0x4b, 0x4d, 0x50].contains(&scan_code)
                        && IS_SWITCHING_APPS.load(Ordering::Acquire)
                    {
                        // arrow keys
                        let reverse = if scan_code == 0x48 || scan_code == 0x4b {
                            1
                        } else {
                            0
                        };
                        send_action_message = Some((id, reverse, false));
                        break;
                    }
                }
            }
        }
    }
    drop(keyboard_state);

    for id in send_done_hotkeys {
        if id == SWITCH_APPS_HOTKEY_ID {
            let _ = queue_message(WM_USER_SWITCH_APPS_DONE, WPARAM(0), LPARAM(0));
            IS_SWITCHING_APPS.store(false, Ordering::Release);
        } else if id == SWITCH_WINDOWS_HOTKEY_ID {
            let _ = queue_message(WM_USER_SWITCH_WINDOWS_DONE, WPARAM(0), LPARAM(0));
        }
    }

    if let Some((id, reverse, is_cancel)) = send_action_message {
        if id == SWITCH_APPS_HOTKEY_ID {
            if is_cancel {
                if !queue_message(WM_USER_SWITCH_APPS_CANCEL, WPARAM(0), LPARAM(0)) {
                    return CallNextHookEx(None, code, w_param, l_param);
                }
                PREVIOUS_KEYCODE.store(scan_code, Ordering::Release);
                IS_SWITCHING_APPS.store(false, Ordering::Release);
            } else {
                if !queue_message(WM_USER_SWITCH_APPS, WPARAM(0), LPARAM(reverse)) {
                    return CallNextHookEx(None, code, w_param, l_param);
                }
                PREVIOUS_KEYCODE.store(scan_code, Ordering::Release);
                IS_SWITCHING_APPS.store(true, Ordering::Release);
            }
            return LRESULT(1);
        } else if id == SWITCH_WINDOWS_HOTKEY_ID {
            if !queue_message(WM_USER_SWITCH_WINDOWS, WPARAM(0), LPARAM(reverse)) {
                return CallNextHookEx(None, code, w_param, l_param);
            }
            PREVIOUS_KEYCODE.store(scan_code, Ordering::Release);
            IS_SWITCHING_APPS.store(false, Ordering::Release);
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, w_param, l_param)
}
