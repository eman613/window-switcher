//! Narrator can replay modifier downs as virtual-key-only injected events.
use super::state::KeyInput;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::*,
    WindowsAndMessaging::{KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP},
};

pub(super) fn decode(data: KBDLLHOOKSTRUCT) -> KeyInput {
    let mut key = KeyInput {
        scan: data.scanCode,
        extended: data.flags.0 & LLKHF_EXTENDED.0 != 0,
        down: data.flags.0 & LLKHF_UP.0 == 0,
    };
    if key.scan == 0 && data.flags.0 & LLKHF_INJECTED.0 != 0 {
        let modifier = match VIRTUAL_KEY(data.vkCode as u16) {
            VK_LMENU => Some((0x38, false)),
            VK_RMENU => Some((0x38, true)),
            VK_MENU => Some((0x38, key.extended)),
            VK_LCONTROL => Some((0x1d, false)),
            VK_RCONTROL => Some((0x1d, true)),
            VK_CONTROL => Some((0x1d, key.extended)),
            VK_LSHIFT => Some((0x2a, false)),
            VK_RSHIFT => Some((0x36, false)),
            VK_LWIN => Some((0x5b, true)),
            VK_RWIN => Some((0x5c, true)),
            _ => None,
        };
        if let Some((scan, extended)) = modifier {
            key.scan = scan;
            key.extended = extended;
            trace!("input stage=modifier-replay scan={scan:#x} extended={extended}");
        }
    }
    key
}
