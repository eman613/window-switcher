//! Draw validated worker-provided ranges without matching or transliteration.
use super::{drawing::text, skin::SearchSkin};
use crate::utils::gdi::SavedDc;
use anyhow::{ensure, Result};
use windows::Win32::{
    Foundation::{RECT, SIZE},
    Graphics::Gdi::*,
};

pub(super) fn draw(
    dc: HDC,
    value: &str,
    ranges: &[std::ops::Range<usize>],
    bounds: RECT,
    color: u32,
    align: DRAW_TEXT_FORMAT,
    skin: &SearchSkin,
) -> Result<()> {
    if ranges.is_empty() {
        return text(dc, &skin.normal, value, bounds, color, align);
    }
    let mut runs = Vec::new();
    let mut previous = 0;
    for range in ranges {
        ensure!(
            range.start >= previous
                && range.start < range.end
                && range.end <= value.len()
                && value.is_char_boundary(range.start)
                && value.is_char_boundary(range.end),
            "search stage=highlight invalid-range"
        );
        if previous < range.start {
            runs.push((&value[previous..range.start], &skin.normal));
        }
        runs.push((&value[range.clone()], &skin.bold));
        previous = range.end;
    }
    if previous < value.len() {
        runs.push((&value[previous..], &skin.normal));
    }
    let saved = SavedDc::new(dc)?;
    let mut total = 0;
    let mut measured = Vec::new();
    for (run, font) in runs {
        saved.select(font.0)?;
        let units: Vec<_> = run.encode_utf16().collect();
        let mut size = SIZE::default();
        ensure!(
            unsafe { GetTextExtentPoint32W(dc, &units, &mut size) }.as_bool(),
            "search stage=highlight-measure failed"
        );
        total += size.cx;
        measured.push((run, font, size.cx));
    }
    let mut left = if align == DT_RIGHT {
        bounds.left.max(bounds.right - total)
    } else {
        bounds.left
    };
    for (run, font, width) in measured {
        if left >= bounds.right {
            break;
        }
        text(dc, font, run, RECT { left, ..bounds }, color, DT_LEFT)?;
        left += width;
    }
    Ok(())
}
