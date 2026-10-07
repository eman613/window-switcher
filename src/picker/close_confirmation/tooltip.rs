use super::*;
use anyhow::ensure;
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        UI::{Controls::*, WindowsAndMessaging::*},
    },
};

pub(in crate::picker) unsafe fn notify(state: &ViewState, lparam: LPARAM) -> Option<LRESULT> {
    if lparam.0 == 0 {
        return None;
    }
    let header = &*(lparam.0 as *const NMHDR);
    if header.hwndFrom != state.close.tooltip.get() || header.code != TTN_SHOW {
        return None;
    }
    debug!("close stage=tooltip-show");
    apply_font(state);
    if let Err(error) = SetWindowPos(
        header.hwndFrom,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    ) {
        warn!("close stage=tooltip-raise code={:#x}", error.code().0);
    }
    Some(LRESULT(0))
}

pub(in crate::picker) fn set_font(state: &ViewState, font: crate::utils::gdi::OwnedGdiObject) {
    // Publish the new handle before releasing the previous native font.
    unsafe {
        SendMessageW(
            state.close.tooltip.get(),
            WM_SETFONT,
            Some(WPARAM(font.0 .0 as usize)),
            None,
        );
    }
    *state.close.hint_font.borrow_mut() = Some(font);
}

fn apply_font(state: &ViewState) {
    if let Ok(font) = state.close.hint_font.try_borrow() {
        if let Some(font) = font.as_ref() {
            unsafe {
                if SendMessageW(state.close.tooltip.get(), WM_GETFONT, None, None).0
                    != font.0 .0 as isize
                {
                    SendMessageW(
                        state.close.tooltip.get(),
                        WM_SETFONT,
                        Some(WPARAM(font.0 .0 as usize)),
                        None,
                    );
                }
            }
        }
    }
}

pub(in crate::picker) fn help(state: &ViewState, hwnd: HWND, text: &str) -> Result<()> {
    let mut units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    if *state.close.help_tip.borrow() == units {
        return Ok(());
    }
    let parent = unsafe { GetParent(hwnd) }?;
    let tool = TTTOOLINFOW {
        cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: parent,
        uId: hwnd.0 as usize,
        lpszText: PWSTR(units.as_mut_ptr()),
        ..Default::default()
    };
    let initial = state.close.help_tip.borrow().is_empty();
    let result = unsafe {
        SendMessageW(
            state.close.tooltip.get(),
            if initial {
                TTM_ADDTOOLW
            } else {
                TTM_UPDATETIPTEXTW
            },
            None,
            Some(LPARAM(&tool as *const _ as isize)),
        )
    };
    ensure!(!initial || result.0 != 0, "close stage=help-tooltip");
    *state.close.help_tip.borrow_mut() = units;
    if initial && !state.dismiss.get().is_invalid() {
        let mut caption: Vec<u16> = state
            .text
            .search_help_toggle()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let button_tool = TTTOOLINFOW {
            uId: state.dismiss.get().0 as usize,
            lpszText: PWSTR(caption.as_mut_ptr()),
            ..tool
        };
        ensure!(
            unsafe {
                SendMessageW(
                    state.close.tooltip.get(),
                    TTM_ADDTOOLW,
                    None,
                    Some(LPARAM(&button_tool as *const _ as isize)),
                )
            }
            .0 != 0,
            "close stage=help-button-tooltip"
        );
        state.close.tips.borrow_mut().push(caption);
    }
    Ok(())
}
