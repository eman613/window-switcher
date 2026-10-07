use super::*;
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
    state.close.tooltip.set(tooltip);
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
    if state.close.updating.get() {
        return;
    }
    let selected = selected(state);
    let selection_changed = state.close.displayed.replace(selected) != selected;
    if selection_changed {
        state.close.cancel();
    }
    if state.close.pending.get().is_some() && state.close.pending.get() != selected {
        state.close.cancel();
    }
    let pending = state.close.pending.get().is_some();
    let target = if pending || state.close.notice.borrow().is_some() {
        selected
    } else {
        targeting::hovered(state)
    };
    if state.close.offered.replace(target) != target || selection_changed {
        for button in buttons(state) {
            let _ =
                unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(Some(button), None, false) };
        }
    }
    if state.kind == crate::picker::ViewKind::Search && state.close.enabled.get() && !pending {
        crate::picker::repaint::hover(state, target.map(|target| target.1));
    }
    let mut row = RECT::default();
    let mut bounds = RECT::default();
    let visible=target.is_some_and(|(_,index,_)|unsafe{SendMessageW(state.list.get(),LB_GETITEMRECT,Some(WPARAM(index)),Some(LPARAM(&mut row as *mut _ as isize)))}.0>=0)
        && unsafe{GetClientRect(state.list.get(),&mut bounds)}.is_ok() && row.top>=0 && row.bottom<=bounds.bottom;
    let list = state.list.get();
    let parent = unsafe { GetParent(list) }.unwrap_or_default();
    if parent.is_invalid() {
        return;
    }
    if visible {
        let layout = crate::picker::row_actions::layout(state, row, pending);
        let (left, width) = (layout.left, layout.width);
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
                state.paint_result(crate::picker::placement::move_child(
                    parent,
                    hwnd,
                    RECT {
                        left: origin.x + x,
                        top: origin.y,
                        right: origin.x + x + width,
                        bottom: origin.y + row.bottom - row.top,
                    },
                ));
                let enabled = state.close.request.get().is_none();
                if windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd).as_bool()
                    != enabled
                {
                    let _ = EnableWindow(hwnd, enabled);
                }
                let font = SendMessageW(list, WM_GETFONT, None, None);
                if SendMessageW(hwnd, WM_GETFONT, None, None) != font {
                    SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
                }
                if show && !IsWindowVisible(hwnd).as_bool() {
                    state.paint_result(
                        SetWindowPos(
                            hwnd,
                            Some(HWND_TOP),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        )
                        .map_err(Into::into),
                    );
                }
                crate::picker::placement::visible(hwnd, show);
            }
        }
        if state.kind == crate::picker::ViewKind::Details
            && (pending || state.close.notice.borrow().is_some())
        {
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
            if state.kind == crate::picker::ViewKind::Details
                && visible
                && (pending || state.close.notice.borrow().is_some())
            {
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
    let colors = state.visual.try_borrow().ok().and_then(|visual| {
        let palette = visual.skin.as_ref()?.palette;
        let selected = selected(state).map(|target| target.1)
            == state.close.offered.get().map(|target| target.1);
        Some(if selected {
            (palette.selected, palette.selected_text)
        } else {
            (palette.hover, palette.text)
        })
    });
    let foreground = colors.map_or(state.foreground.get(), |(_, color)| {
        crate::text_raster::colorref(color)
    });
    unsafe {
        if let Some((background, _)) = colors {
            crate::picker::drawing::rounded(item.hDC, rect, 0, background, None)?;
        } else {
            ensure!(
                FillRect(item.hDC, &rect, state.brush.get()) != 0,
                "close stage=button-background"
            );
        }
        SetBkMode(item.hDC, TRANSPARENT);
        SetTextColor(
            item.hDC,
            if item.itemState.0 & ODS_DISABLED.0 != 0 {
                state.muted.get()
            } else {
                foreground
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
                colors.map_or(state.background.get().0, |(background, _)| background),
                Some(colors.map_or(state.foreground.get().0, |(_, foreground)| foreground)),
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
