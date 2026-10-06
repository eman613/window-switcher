use super::*;
use anyhow::ensure;
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{LPARAM, LRESULT},
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
    Ok(())
}
