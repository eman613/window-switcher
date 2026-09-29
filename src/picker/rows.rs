use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use anyhow::{Context, Result};
use windows::Win32::{
    Foundation::{LPARAM, RECT, WPARAM},
    Graphics::Gdi::InvalidateRect,
    UI::WindowsAndMessaging::{
        GetClientRect, SendMessageW, LB_GETITEMRECT, LB_GETTOPINDEX, LB_SETTOPINDEX,
    },
};

use super::{skin::SearchSkin, PickerWindow};
use crate::icon_cache::{CachedIcon, IconKey};

pub(crate) struct PickerRow {
    pub key: IconKey,
    pub primary: String,
    pub secondary: String,
    pub meta: String,
    pub icon: Option<Arc<CachedIcon>>,
    pub remembered: Weak<CachedIcon>,
}

impl PickerRow {
    fn accessible_label(&self) -> String {
        let value = if self.meta.is_empty() {
            format!("{} — {}", self.primary, self.secondary)
        } else {
            format!("{} — {}，{}", self.primary, self.secondary, self.meta)
        };
        super::label(&value)
    }

    pub(super) fn image(&self) -> Option<Arc<CachedIcon>> {
        self.icon.clone().or_else(|| self.remembered.upgrade())
    }
}

#[derive(Default)]
pub(super) struct PickerVisual {
    pub skin: Option<SearchSkin>,
    pub rows: Vec<PickerRow>,
    pub query: String,
}

impl PickerWindow {
    pub(crate) fn highlight_query(&self, query: &str) {
        if let Ok(mut visual) = self.state().visual.try_borrow_mut() {
            visual.query = query.to_owned();
        }
    }
    pub(crate) fn replace_rows(
        &self,
        mut rows: Vec<PickerRow>,
        selected: usize,
        epoch: u64,
    ) -> Result<()> {
        let labels = rows
            .iter()
            .map(PickerRow::accessible_label)
            .collect::<Vec<_>>();
        let top = unsafe { SendMessageW(self.controls().list, LB_GETTOPINDEX, None, None) }
            .0
            .max(0) as usize;
        let mut restore_top = None;
        let mut unchanged = false;
        {
            let mut visual = self
                .state()
                .visual
                .try_borrow_mut()
                .context("picker stage=rows reentrant update")?;
            if !self.state().reset_scroll.get() {
                unchanged = visual.rows.len() == rows.len()
                    && visual.rows.iter().zip(&rows).all(|(old, new)| {
                        old.key == new.key && old.accessible_label() == new.accessible_label()
                    });
                restore_top = visual
                    .rows
                    .get(top)
                    .and_then(|anchor| rows.iter().position(|row| row.key == anchor.key));
            }
            let old: HashMap<_, _> = visual
                .rows
                .iter()
                .map(|row| (row.key.clone(), (row.icon.clone(), row.remembered.clone())))
                .collect();
            for row in &mut rows {
                if let Some((icon, remembered)) = old.get(&row.key) {
                    row.icon = icon.clone();
                    row.remembered = remembered.clone();
                }
            }
            visual.rows = rows;
        }
        if unchanged {
            debug!("search stage=refresh retained-viewport top={top}");
            return self.finish_rows(epoch, labels.is_empty());
        }
        self.replace(&labels, selected, epoch)?;
        if let Some(top) = restore_top {
            unsafe {
                SendMessageW(
                    self.controls().list,
                    LB_SETTOPINDEX,
                    Some(WPARAM(top)),
                    None,
                );
            }
            super::scrollbar::invalidate(self.state());
        }
        Ok(())
    }

    pub(crate) fn visible_icon_keys(&self) -> Vec<IconKey> {
        if !self.visible() || self.state().busy.get() {
            return Vec::new();
        }
        let list = self.controls().list;
        let top = unsafe { SendMessageW(list, LB_GETTOPINDEX, None, None) }
            .0
            .max(0) as usize;
        let mut rect = RECT::default();
        if unsafe { GetClientRect(list, &mut rect) }.is_err() {
            return Vec::new();
        }
        let Ok(mut visual) = self.state().visual.try_borrow_mut() else {
            return Vec::new();
        };
        let row_height = visual
            .skin
            .as_ref()
            .map_or(64, |skin| skin.row_height)
            .max(1);
        let count = (rect.bottom.max(0) + row_height - 1) / row_height;
        let end = top.saturating_add(count as usize).min(visual.rows.len());
        let mut keys = Vec::with_capacity(count as usize);
        for (index, row) in visual.rows.iter_mut().enumerate() {
            if index >= top && index < end {
                keys.push(row.key.clone());
            } else {
                row.icon = None;
            }
        }
        keys
    }

    pub(crate) fn apply_icon(&self, key: &IconKey, icon: Arc<CachedIcon>) {
        let updated = {
            let Ok(mut visual) = self.state().visual.try_borrow_mut() else {
                return;
            };
            let Some((index, row)) = visual
                .rows
                .iter_mut()
                .enumerate()
                .find(|(_, row)| row.key == *key)
            else {
                return;
            };
            let changed = row
                .image()
                .is_none_or(|previous| previous.revision != icon.revision);
            row.remembered = Arc::downgrade(&icon);
            row.icon = Some(icon);
            changed.then_some(index)
        };
        if let Some(index) = updated {
            let mut rect = RECT::default();
            let list = self.controls().list;
            if unsafe {
                SendMessageW(
                    list,
                    LB_GETITEMRECT,
                    Some(WPARAM(index)),
                    Some(LPARAM((&mut rect as *mut RECT) as isize)),
                )
            }
            .0 >= 0
            {
                let _ = unsafe { InvalidateRect(Some(list), Some(&rect), false) };
            }
        }
    }
}
