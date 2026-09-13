use crate::{
    app::{
        WM_USER_KEYBOARD_QUEUE, WM_USER_SWITCH_APPS, WM_USER_SWITCH_APPS_CANCEL,
        WM_USER_SWITCH_APPS_DONE, WM_USER_SWITCH_WINDOWS, WM_USER_SWITCH_WINDOWS_DONE,
    },
    config::{Hotkey, SWITCH_APPS_HOTKEY_ID, SWITCH_WINDOWS_HOTKEY_ID},
    foreground::IS_FOREGROUND_IN_BLACKLIST,
    metrics::{enabled as performance_metrics_enabled, record_keyboard_elapsed},
};

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicIsize, Ordering},
        mpsc, LazyLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{SCANCODE_LSHIFT, SCANCODE_RSHIFT},
        WindowsAndMessaging::{
            CallNextHookEx, GetMessageW, PeekMessageW, PostMessageW, PostThreadMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, LLKHF_UP, MSG, PM_NOREMOVE,
            WH_KEYBOARD_LL, WM_QUIT,
        },
    },
};

static KEYBOARD_STATE: LazyLock<Mutex<KeyboardStateMachine>> =
    LazyLock::new(|| Mutex::new(KeyboardStateMachine::new()));
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static KEYBOARD_MESSAGES: LazyLock<Mutex<VecDeque<KeyboardMessage>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(KEYBOARD_QUEUE_CAPACITY)));
static KEYBOARD_WAKE_PENDING: AtomicIsize = AtomicIsize::new(0);

const KEYBOARD_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyboardMessage {
    pub(crate) msg: u32,
    pub(crate) wparam: WPARAM,
    pub(crate) lparam: LPARAM,
    pub(crate) sequence: u64,
    pub(crate) captured_at: Option<Instant>,
}

#[derive(Debug)]
pub struct KeyboardListener {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
}

impl KeyboardListener {
    pub fn init(hwnd: HWND, hotkeys: &[&Hotkey]) -> Result<Self> {
        WINDOW.store(hwnd.0 as isize, Ordering::Release);
        clear_keyboard_messages();

        let keyboard_state = KeyboardStateMachine {
            hotkeys: hotkeys
                .iter()
                .map(|hotkey| HotKeyState {
                    hotkey: (*hotkey).clone(),
                })
                .collect(),
            ..KeyboardStateMachine::new()
        };
        *KEYBOARD_STATE.lock() = keyboard_state;

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("window-switcher-keyboard-hook".to_string())
            .spawn(move || run_hook_thread(ready_tx))
            .map_err(|err| anyhow!("Failed to start keyboard hook thread, {err}"))?;
        let thread_id = match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = worker.join();
                WINDOW.store(0, Ordering::Release);
                *KEYBOARD_STATE.lock() = KeyboardStateMachine::new();
                return Err(anyhow!(error));
            }
            Err(error) => {
                let _ = worker.join();
                WINDOW.store(0, Ordering::Release);
                *KEYBOARD_STATE.lock() = KeyboardStateMachine::new();
                return Err(anyhow!(
                    "Keyboard hook thread did not become ready: {error}"
                ));
            }
        };
        info!("keyboard listener start thread_id={thread_id}");

        Ok(Self {
            thread_id,
            worker: Some(worker),
        })
    }
}

impl Drop for KeyboardListener {
    fn drop(&mut self) {
        debug!("keyboard listener destroyed");
        if self.thread_id != 0 {
            if let Err(error) =
                unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            {
                warn!(
                    "failed to stop keyboard hook thread {}: {error}",
                    self.thread_id
                );
            }
        }
        if let Some(worker) = self.worker.take() {
            if let Err(error) = worker.join() {
                warn!("keyboard hook thread panicked during cleanup: {error:?}");
            }
        }
        WINDOW.store(0, Ordering::Release);
        *KEYBOARD_STATE.lock() = KeyboardStateMachine::new();
        clear_keyboard_messages();
    }
}

fn run_hook_thread(ready: mpsc::SyncSender<std::result::Result<u32, String>>) {
    let hook = unsafe {
        let hinstance = match GetModuleHandleW(None) {
            Ok(hinstance) => hinstance,
            Err(error) => {
                let _ = ready.send(Err(format!("Failed to get module handle, {error}")));
                return;
            }
        };
        match SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_proc),
            Some(hinstance.into()),
            0,
        ) {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready.send(Err(format!("Failed to set windows hook, {error}")));
                return;
            }
        }
    };
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    if ready.send(Ok(thread_id)).is_err() {
        unsafe { UnhookWindowsHookEx(hook) }.ok();
        return;
    }

    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
    }
    unsafe { UnhookWindowsHookEx(hook) }.ok();
}

#[derive(Debug)]
struct HotKeyState {
    hotkey: Hotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveSequence {
    id: u32,
    sequence: u64,
}

#[derive(Debug)]
struct KeyboardStateMachine {
    hotkeys: Vec<HotKeyState>,
    pressed_modifiers: [bool; 256],
    shift_pressed: bool,
    active: Option<ActiveSequence>,
    next_sequence: u64,
}

#[derive(Debug, Default)]
struct KeyboardTransition {
    messages: Vec<KeyboardMessage>,
    intercept: bool,
    reset_on_delivery_failure: bool,
}

impl KeyboardStateMachine {
    fn new() -> Self {
        Self {
            hotkeys: Vec::new(),
            pressed_modifiers: [false; 256],
            shift_pressed: false,
            active: None,
            next_sequence: 0,
        }
    }

    fn reset_active(&mut self) {
        self.active = None;
    }

    fn handle(
        &mut self,
        scan_code: u32,
        extended: bool,
        pressed: bool,
        switch_windows_allowed: bool,
    ) -> KeyboardTransition {
        let index = modifier_index(scan_code, extended);
        if index < self.pressed_modifiers.len() {
            self.pressed_modifiers[index] = pressed;
        }
        if [SCANCODE_LSHIFT, SCANCODE_RSHIFT].contains(&scan_code) {
            self.shift_pressed = self.any_modifier_down(scan_code);
        }

        if self
            .hotkeys
            .iter()
            .any(|state| state.hotkey.modifier.contains(&scan_code))
        {
            if !pressed {
                return self.handle_modifier_release(scan_code);
            }
            return KeyboardTransition::default();
        }
        if !pressed {
            return KeyboardTransition::default();
        }

        let Some(active) = self.active else {
            return self.handle_initial_key(scan_code, switch_windows_allowed);
        };
        let active_modifiers = self
            .hotkeys
            .iter()
            .find(|state| state.hotkey.id == active.id)
            .map(|state| self.modifiers_down(&state.hotkey.modifier))
            .unwrap_or(false);
        if !active_modifiers {
            self.active = None;
            return KeyboardTransition::default();
        }
        if active.id == SWITCH_APPS_HOTKEY_ID {
            if scan_code == 0x01 {
                self.active = None;
                return KeyboardTransition {
                    messages: vec![keyboard_message(
                        WM_USER_SWITCH_APPS_CANCEL,
                        0,
                        active.sequence,
                    )],
                    intercept: true,
                    reset_on_delivery_failure: true,
                };
            }
            if [0x48, 0x4b, 0x4d, 0x50].contains(&scan_code) {
                let reverse = matches!(scan_code, 0x48 | 0x4b);
                return KeyboardTransition {
                    messages: vec![keyboard_message(
                        WM_USER_SWITCH_APPS,
                        reverse as isize,
                        active.sequence,
                    )],
                    intercept: true,
                    reset_on_delivery_failure: true,
                };
            }
        }
        KeyboardTransition::default()
    }

    fn handle_initial_key(
        &mut self,
        scan_code: u32,
        switch_windows_allowed: bool,
    ) -> KeyboardTransition {
        for state in &self.hotkeys {
            if state.hotkey.code != scan_code || !self.modifiers_down(&state.hotkey.modifier) {
                continue;
            }
            if state.hotkey.modifier.contains(&0x38) && self.is_altgr_down() {
                continue;
            }
            let reverse = self.shift_pressed as isize;
            let message = if state.hotkey.id == SWITCH_APPS_HOTKEY_ID {
                WM_USER_SWITCH_APPS
            } else if state.hotkey.id == SWITCH_WINDOWS_HOTKEY_ID {
                if !switch_windows_allowed {
                    continue;
                }
                WM_USER_SWITCH_WINDOWS
            } else {
                continue;
            };
            self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
            self.active = Some(ActiveSequence {
                id: state.hotkey.id,
                sequence: self.next_sequence,
            });
            return KeyboardTransition {
                messages: vec![keyboard_message(message, reverse, self.next_sequence)],
                intercept: true,
                reset_on_delivery_failure: true,
            };
        }
        KeyboardTransition::default()
    }

    fn handle_modifier_release(&mut self, scan_code: u32) -> KeyboardTransition {
        let Some(active) = self.active else {
            return KeyboardTransition::default();
        };
        let Some(state) = self
            .hotkeys
            .iter()
            .find(|state| state.hotkey.id == active.id)
        else {
            self.active = None;
            return KeyboardTransition::default();
        };
        if state.hotkey.modifier.contains(&scan_code)
            && !self.modifiers_down(&state.hotkey.modifier)
        {
            self.active = None;
            let message = if active.id == SWITCH_APPS_HOTKEY_ID {
                WM_USER_SWITCH_APPS_DONE
            } else {
                WM_USER_SWITCH_WINDOWS_DONE
            };
            return KeyboardTransition {
                messages: vec![keyboard_message(message, 0, active.sequence)],
                ..KeyboardTransition::default()
            };
        }
        KeyboardTransition::default()
    }

    fn modifiers_down(&self, modifiers: &[u32; 2]) -> bool {
        modifiers
            .iter()
            .any(|modifier| self.any_modifier_down(*modifier))
    }

    fn any_modifier_down(&self, scan_code: u32) -> bool {
        (0..2).any(|extended| {
            let index = modifier_index(scan_code, extended != 0);
            index < self.pressed_modifiers.len() && self.pressed_modifiers[index]
        })
    }

    fn is_altgr_down(&self) -> bool {
        self.pressed_modifiers[modifier_index(0x1d, true)]
            && self.pressed_modifiers[modifier_index(0x38, true)]
    }
}

fn modifier_index(scan_code: u32, extended: bool) -> usize {
    scan_code.saturating_mul(2) as usize + usize::from(extended)
}

fn keyboard_message(msg: u32, lparam: isize, sequence: u64) -> KeyboardMessage {
    KeyboardMessage {
        msg,
        wparam: WPARAM(0),
        lparam: LPARAM(lparam),
        sequence,
        captured_at: None,
    }
}

fn queue_message(message: KeyboardMessage) -> bool {
    let Some(mut queue) = KEYBOARD_MESSAGES.try_lock() else {
        debug!(
            "keyboard message queue lock unavailable; dropping message {}",
            message.msg
        );
        return false;
    };

    if !enqueue_message(&mut queue, message) {
        debug!(
            "keyboard message queue full; dropping message {}",
            message.msg
        );
        return false;
    }
    drop(queue);

    if KEYBOARD_WAKE_PENDING.swap(1, Ordering::AcqRel) == 0 && !post_keyboard_wake() {
        drop_queued_messages();
        return false;
    }
    true
}

fn enqueue_message(queue: &mut VecDeque<KeyboardMessage>, message: KeyboardMessage) -> bool {
    if queue.len() >= KEYBOARD_QUEUE_CAPACITY {
        if is_completion_message(message.msg) {
            if let Some(index) = queue
                .iter()
                .position(|queued| !is_completion_message(queued.msg))
            {
                queue.remove(index);
            } else {
                // Keep the newest terminal event. An older terminal event is
                // no longer actionable once this event has been observed.
                queue.pop_front();
            }
        } else {
            return false;
        }
    }
    queue.push_back(message);
    true
}

fn post_keyboard_wake() -> bool {
    let raw_hwnd = WINDOW.load(Ordering::Acquire);
    if raw_hwnd == 0 {
        KEYBOARD_WAKE_PENDING.store(0, Ordering::Release);
        return false;
    }

    let hwnd = HWND(raw_hwnd as _);
    if unsafe { PostMessageW(Some(hwnd), WM_USER_KEYBOARD_QUEUE, WPARAM(0), LPARAM(0)) }.is_err() {
        KEYBOARD_WAKE_PENDING.store(0, Ordering::Release);
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

    if performance_metrics_enabled() {
        for message in &messages {
            if let Some(captured_at) = message.captured_at {
                record_keyboard_elapsed(
                    "keyboard_queue_wait",
                    message.sequence,
                    captured_at.elapsed(),
                );
            }
        }
    }

    KEYBOARD_WAKE_PENDING.store(0, Ordering::Release);
    let has_pending = !KEYBOARD_MESSAGES.lock().is_empty();
    if has_pending && KEYBOARD_WAKE_PENDING.swap(1, Ordering::AcqRel) == 0 && !post_keyboard_wake()
    {
        drop_queued_messages();
    }
    messages
}

fn clear_keyboard_messages() {
    KEYBOARD_MESSAGES.lock().clear();
    KEYBOARD_WAKE_PENDING.store(0, Ordering::Release);
}

fn drop_queued_messages() {
    if let Some(mut queue) = KEYBOARD_MESSAGES.try_lock() {
        queue.clear();
    }
    KEYBOARD_WAKE_PENDING.store(0, Ordering::Release);
}

unsafe extern "system" fn keyboard_proc(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code < 0 || l_param.0 == 0 {
        return CallNextHookEx(None, code, w_param, l_param);
    }

    let Some(kbd_data) = (l_param.0 as *const KBDLLHOOKSTRUCT).as_ref() else {
        return CallNextHookEx(None, code, w_param, l_param);
    };
    let hook_started = performance_metrics_enabled().then(Instant::now);
    let scan_code = kbd_data.scanCode;
    let is_key_pressed = kbd_data.flags.0 & LLKHF_UP.0 == 0;
    let extended = kbd_data.flags.0 & 0x01 != 0;
    let Some(mut keyboard_state) = KEYBOARD_STATE.try_lock() else {
        return CallNextHookEx(None, code, w_param, l_param);
    };
    let transition = keyboard_state.handle(
        scan_code,
        extended,
        is_key_pressed,
        !IS_FOREGROUND_IN_BLACKLIST.load(Ordering::Acquire),
    );
    drop(keyboard_state);

    let KeyboardTransition {
        messages,
        intercept,
        reset_on_delivery_failure,
    } = transition;
    let mut delivery_failed = false;
    let captured_at = hook_started;
    let message_sequences = performance_metrics_enabled().then(|| {
        messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>()
    });
    for mut message in messages {
        message.captured_at = captured_at;
        let enqueue_started = performance_metrics_enabled().then(Instant::now);
        let sequence = message.sequence;
        let queued = queue_message(message);
        if !queued {
            delivery_failed = true;
        } else if let Some(enqueue_started) = enqueue_started {
            record_keyboard_elapsed("keyboard_enqueue", sequence, enqueue_started.elapsed());
        }
    }
    if let (Some(hook_started), Some(message_sequences)) = (hook_started, message_sequences) {
        for sequence in message_sequences {
            record_keyboard_elapsed("keyboard_hook", sequence, hook_started.elapsed());
        }
    }
    if delivery_failed && reset_on_delivery_failure {
        KEYBOARD_STATE.lock().reset_active();
    }
    if intercept && !delivery_failed {
        return LRESULT(1);
    }
    CallNextHookEx(None, code, w_param, l_param)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> KeyboardStateMachine {
        let apps = Hotkey::create(SWITCH_APPS_HOTKEY_ID, "apps", "alt + tab").unwrap();
        let windows = Hotkey::create(SWITCH_WINDOWS_HOTKEY_ID, "windows", "alt + `").unwrap();
        KeyboardStateMachine {
            hotkeys: vec![
                HotKeyState { hotkey: apps },
                HotKeyState { hotkey: windows },
            ],
            ..KeyboardStateMachine::new()
        }
    }

    #[test]
    fn direction_navigation_keeps_sequence_until_modifier_release() {
        let mut state = machine();
        assert!(state.handle(0x38, false, true, true).messages.is_empty());
        let first = state.handle(0x0f, false, true, true);
        assert_eq!(first.messages[0].msg, WM_USER_SWITCH_APPS);
        let next = state.handle(0x4d, true, true, true);
        assert_eq!(next.messages[0].msg, WM_USER_SWITCH_APPS);
        let done = state.handle(0x38, false, false, true);
        assert_eq!(done.messages[0].msg, WM_USER_SWITCH_APPS_DONE);
        assert_eq!(done.messages[0].sequence, first.messages[0].sequence);
    }

    #[test]
    fn escape_is_only_handled_for_an_active_apps_sequence() {
        let mut state = machine();
        assert!(state.handle(0x01, false, true, true).messages.is_empty());
        state.handle(0x38, false, true, true);
        state.handle(0x0f, false, true, true);
        let cancel = state.handle(0x01, false, true, true);
        assert_eq!(cancel.messages[0].msg, WM_USER_SWITCH_APPS_CANCEL);
        assert!(state.handle(0x38, false, false, true).messages.is_empty());
    }

    #[test]
    fn both_sides_of_modifier_must_be_released() {
        let mut state = machine();
        state.handle(0x38, false, true, true);
        state.handle(0x0f, false, true, true);
        state.handle(0x38, true, true, true);
        assert!(state.handle(0x38, false, false, true).messages.is_empty());
        let done = state.handle(0x38, true, false, true);
        assert_eq!(done.messages[0].msg, WM_USER_SWITCH_APPS_DONE);
    }

    #[test]
    fn blocked_windows_hotkey_does_not_start_a_sequence() {
        let mut state = machine();
        state.handle(0x38, false, true, true);
        assert!(state.handle(0x29, false, true, false).messages.is_empty());
        assert!(state.active.is_none());
    }

    #[test]
    fn altgr_does_not_trigger_plain_alt_hotkey() {
        let mut state = machine();
        state.handle(0x1d, true, true, true);
        state.handle(0x38, true, true, true);
        assert!(state.handle(0x0f, false, true, true).messages.is_empty());
        assert!(state.active.is_none());
    }

    #[test]
    fn full_queue_preserves_terminal_events() {
        let mut queue = VecDeque::new();
        for index in 0..KEYBOARD_QUEUE_CAPACITY {
            assert!(enqueue_message(
                &mut queue,
                keyboard_message(WM_USER_SWITCH_APPS, index as isize, 1),
            ));
        }
        assert!(!enqueue_message(
            &mut queue,
            keyboard_message(WM_USER_SWITCH_APPS, 0, 1),
        ));
        assert!(enqueue_message(
            &mut queue,
            keyboard_message(WM_USER_SWITCH_APPS_DONE, 0, 1),
        ));
        assert_eq!(queue.len(), KEYBOARD_QUEUE_CAPACITY);
        assert!(queue
            .iter()
            .any(|message| message.msg == WM_USER_SWITCH_APPS_DONE));
    }
}
