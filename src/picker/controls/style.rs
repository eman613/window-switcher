//! Transactional replacement of picker style resources.
use super::*;

impl Controls {
    pub(in crate::picker) fn style(
        &mut self,
        parent: HWND,
        state: &ViewState,
        config: &Config,
        dpi: u32,
    ) -> Result<()> {
        let mut signature = config.clone();
        signature.search_width = 0;
        signature.search_visible_rows = 0;
        if !state.style_dirty.get()
            && self
                .styled
                .as_ref()
                .is_some_and(|(previous, previous_dpi)| {
                    previous == &signature && *previous_dpi == dpi
                })
        {
            return Ok(());
        }
        state.paint_error.set(false);
        if state.kind == ViewKind::Search {
            let skin = SearchSkin::new(config, dpi)?;
            // Acquire the fallible borrow before publishing any native handles.
            // Retain the old skin until every control has its replacement font.
            let mut visual = state
                .visual
                .try_borrow_mut()
                .context("picker stage=style reentrant update")?;
            state.scroll_mode.set(
                if skin.palette.high_contrast
                    && config.search_scrollbar == crate::config::ScrollBarMode::Auto
                {
                    crate::config::ScrollBarMode::Always
                } else {
                    config.search_scrollbar
                },
            );
            state
                .background
                .set(crate::text_raster::colorref(skin.palette.surface));
            state
                .foreground
                .set(crate::text_raster::colorref(skin.palette.text));
            state
                .muted
                .set(crate::text_raster::colorref(skin.palette.muted));
            state.brush.set(skin.background_brush());
            for hwnd in [
                self.label,
                self.results_label,
                self.list,
                self.status,
                self.help,
                self.clear,
                self.dismiss,
            ] {
                Self::set_font(hwnd, &skin.normal);
            }
            Self::set_font(self.edit, &skin.input);
            Self::set_font(self.help, &skin.help_font);
            Self::set_font(self.notice, &skin.title);
            visual.skin = Some(skin);
        } else {
            let appearance = Appearance::capture(config);
            let color = crate::text_raster::colorref(appearance.panel.color);
            let brush = OwnedGdiObject::new(
                HGDIOBJ(unsafe { CreateSolidBrush(color) }.0),
                "search-background",
            )?;
            #[cfg(test)]
            fail_style_at(1)?;
            let font = content_font(config, dpi, 14)?;
            #[cfg(test)]
            fail_style_at(2)?;
            let mut metrics = LOGFONTW::default();
            ensure!(
                unsafe {
                    GetObjectW(
                        font.0,
                        std::mem::size_of::<LOGFONTW>() as i32,
                        Some((&mut metrics as *mut LOGFONTW).cast()),
                    )
                } == std::mem::size_of::<LOGFONTW>() as i32,
                "picker stage=content-font-metrics failed"
            );
            state.background.set(color);
            state
                .foreground
                .set(crate::text_raster::colorref(appearance.text));
            state.muted.set(state.foreground.get());
            state.brush.set(HBRUSH(brush.0 .0));
            self.text_height.set(metrics.lfHeight.abs());
            for hwnd in [self.label, self.list, self.status, self.back] {
                Self::set_font(hwnd, &font);
            }
            self.font = Some(font);
            self.background = Some(brush);
        }
        state.dpi.set(dpi);
        self.styled = Some((signature, dpi));
        state.style_dirty.set(false);
        let _ = unsafe { InvalidateRect(Some(parent), None, true) };
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static STYLE_FAILURE: Cell<u8> = const { Cell::new(0) };
}

#[cfg(test)]
fn fail_style_at(stage: u8) -> Result<()> {
    ensure!(
        !STYLE_FAILURE.with(|failure| failure.get() == stage && failure.replace(0) == stage),
        "picker stage=style injected-resource-failure stage={stage}"
    );
    Ok(())
}

#[cfg(test)]
#[path = "style_tests.rs"]
mod tests;
