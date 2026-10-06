use super::*;
use crate::picker::skin::px;
use anyhow::ensure;
use windows::{
    core::{w, HSTRING, PWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{FillRect, SetBkMode, SetTextColor, TRANSPARENT},
        System::SystemServices::{SS_CENTERIMAGE, SS_ENDELLIPSIS, SS_NOPREFIX},
        UI::{
            Controls::{
                DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, TTF_IDISHWND, TTF_SUBCLASS,
                TTM_ADDTOOLW, TTTOOLINFOW,
            },
            Input::KeyboardAndMouse::EnableWindow,
            WindowsAndMessaging::*,
        },
    },
};

pub(in crate::picker) fn create(parent: HWND, state: &ViewState) -> Result<()> {
    let labels = [
        (CLOSE, state.text.close_window()),
        (YES, state.text.close_yes()),
        (NO, state.text.close_cancel()),
    ];
    use windows::Win32::UI::Controls::{
        InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, TTS_ALWAYSTIP,
    };
    ensure!(
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_WIN95_CLASSES,
            })
        }
        .as_bool(),
        "close stage=common-controls"
    );
    let tooltip = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            w!("tooltips_class32"),
            None,
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP),
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            None,
            None,
        )
    }?;
    for (id, title) in labels {
        let control = crate::picker::controls::child(
            parent,
            w!("BUTTON"),
            title,
            WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            id,
        )?;
        match id {
            CLOSE => state.close.close.set(control),
            YES => state.close.yes.set(control),
            _ => state.close.no.set(control),
        }
        let mut text = title.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let tool = TTTOOLINFOW {
            // V2 is accepted by both the classic and themed common controls.
            cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: parent,
            uId: control.0 as usize,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        ensure!(
            unsafe {
                SendMessageW(
                    tooltip,
                    TTM_ADDTOOLW,
                    None,
                    Some(LPARAM(&tool as *const _ as isize)),
                )
            }
            .0 != 0,
            "close stage=tooltip"
        );
        state.close.tips.borrow_mut().push(text);
        ensure!(
            unsafe {
                windows::Win32::UI::Shell::SetWindowSubclass(
                    control,
                    Some(crate::picker::messages::control_proc),
                    1,
                    state as *const _ as usize,
                )
            }
            .as_bool(),
            "close stage=subclass"
        );
        unsafe {
            let _ = ShowWindow(control, SW_HIDE);
        }
    }
    state.close.label.set(crate::picker::controls::child(
        parent,
        w!("STATIC"),
        "",
        WINDOW_STYLE(SS_NOPREFIX.0 | SS_CENTERIMAGE.0 | SS_ENDELLIPSIS.0),
        LABEL,
    )?);
    unsafe {
        let _ = ShowWindow(state.close.label.get(), SW_HIDE);
    }
    Ok(())
}
pub(in crate::picker) fn refresh(state: &ViewState) {
    let selected = selected(state);
    if state.close.displayed.replace(selected) != selected {
        state.close.cancel();
    }
    if state.close.pending.get().is_some() && state.close.pending.get() != selected {
        state.close.cancel();
    }
    let pending = state.close.pending.get().is_some();
    let mut row = RECT::default();
    let mut bounds = RECT::default();
    let visible=selected.is_some_and(|(_,index,_)|unsafe{SendMessageW(state.list.get(),LB_GETITEMRECT,Some(WPARAM(index)),Some(LPARAM(&mut row as *mut _ as isize)))}.0>=0)
        && unsafe{GetClientRect(state.list.get(),&mut bounds)}.is_ok() && row.top>=0 && row.bottom<=bounds.bottom;
    let list = state.list.get();
    let parent = unsafe { GetParent(list) }.unwrap_or_default();
    if parent.is_invalid() {
        return;
    }
    if visible {
        let width = px(28, state.dpi.get()).min((row.bottom - row.top).max(1));
        let left = if state.kind == crate::picker::ViewKind::Search {
            bounds.right - width * if pending { 2 } else { 1 } - px(17, state.dpi.get())
        } else {
            let mut outer = RECT::default();
            if unsafe { GetWindowRect(list, &mut outer) }.is_err() {
                return;
            }
            outer.right - outer.left + px(3, state.dpi.get())
        };
        // Convert from list client to picker client; handles DPI/position changes.
        let mut origin = windows::Win32::Foundation::POINT { x: 0, y: row.top };
        unsafe {
            windows::Win32::Graphics::Gdi::MapWindowPoints(
                Some(list),
                Some(parent),
                std::slice::from_mut(&mut origin),
            );
        }
        for (hwnd, x, show) in [
            (state.close.close.get(), left, !pending),
            (state.close.yes.get(), left, pending),
            (state.close.no.get(), left + width, pending),
        ] {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    origin.x + x,
                    origin.y,
                    width,
                    row.bottom - row.top,
                    SWP_NOACTIVATE,
                );
                let _ = EnableWindow(hwnd, state.close.request.get().is_none());
                let font = SendMessageW(list, WM_GETFONT, None, None);
                SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
                let _ = ShowWindow(hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        if pending || state.close.notice.borrow().is_some() {
            if let Some((_, index, _)) = selected {
                if let Some((_, title)) = state.close.targets.borrow().get(index) {
                    let question = state
                        .close
                        .notice
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| state.text.close_question(title));
                    unsafe {
                        let _ = SetWindowTextW(state.close.label.get(), &HSTRING::from(question));
                    }
                }
            }
            unsafe {
                let _ = SetWindowPos(
                    state.close.label.get(),
                    Some(HWND_TOP),
                    origin.x,
                    origin.y,
                    left.min(bounds.right).max(1),
                    row.bottom - row.top,
                    SWP_NOACTIVATE,
                );
                let font = SendMessageW(list, WM_GETFONT, None, None);
                SendMessageW(
                    state.close.label.get(),
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    None,
                );
            }
        }
    }
    unsafe {
        let _ = ShowWindow(
            state.close.label.get(),
            if visible && (pending || state.close.notice.borrow().is_some()) {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            },
        );
        if !visible {
            for hwnd in [
                state.close.close.get(),
                state.close.yes.get(),
                state.close.no.get(),
            ] {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}
pub(in crate::picker) fn draw(state: &ViewState, item: &DRAWITEMSTRUCT) -> Result<bool> {
    let glyph = match item.CtlID as usize {
        CLOSE => "×",
        YES => "✓",
        NO => "↶",
        _ => return Ok(false),
    };
    let mut rect = item.rcItem;
    unsafe {
        ensure!(
            FillRect(item.hDC, &rect, state.brush.get()) != 0,
            "close stage=button-background"
        );
        SetBkMode(item.hDC, TRANSPARENT);
        SetTextColor(
            item.hDC,
            if item.itemState.0 & ODS_DISABLED.0 != 0 {
                state.muted.get()
            } else {
                state.foreground.get()
            },
        );
        let active = item.itemState.0 & ODS_SELECTED.0 != 0;
        if (active || state.hot_control.get() == item.hwndItem)
            && item.itemState.0 & ODS_DISABLED.0 == 0
        {
            let inset = if active { 3 } else { 1 };
            let outline = RECT {
                left: rect.left + inset,
                top: rect.top + inset,
                right: rect.right - inset,
                bottom: rect.bottom - inset,
            };
            crate::picker::drawing::rounded(
                item.hDC,
                outline,
                0,
                state.background.get().0,
                Some(state.foreground.get().0),
            )?;
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            ensure!(
                windows::Win32::Graphics::Gdi::DrawFocusRect(item.hDC, &rect).as_bool(),
                "close stage=button-focus"
            );
        }
        let mut units = glyph.encode_utf16().collect::<Vec<_>>();
        ensure!(
            windows::Win32::Graphics::Gdi::DrawTextW(
                item.hDC,
                &mut units,
                &mut rect,
                windows::Win32::Graphics::Gdi::DT_CENTER
                    | windows::Win32::Graphics::Gdi::DT_VCENTER
                    | windows::Win32::Graphics::Gdi::DT_SINGLELINE
            ) != 0,
            "close stage=button-glyph"
        );
    }
    Ok(true)
}
