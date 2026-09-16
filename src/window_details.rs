//! A details view retains only the current group's bounded snapshot records.
use crate::{
    config::Config,
    layout::MonitorSnapshot,
    localization::Text,
    picker::{messages, PickerWindow, ViewKind},
    utils::window_identity::WindowIdentity,
    window_snapshot::WindowRecord,
    window_target::WindowTarget,
};
use anyhow::Result;
use std::sync::Arc;
use windows::Win32::Foundation::HWND;

pub(crate) enum DetailsAction {
    Cancel,
    Back,
    Activate(WindowRecord),
}

pub(crate) struct WindowDetails {
    window: PickerWindow,
    text: Text,
    group: Arc<str>,
    records: Arc<[WindowRecord]>,
    epoch: u64,
}

impl WindowDetails {
    pub(crate) fn new(owner: HWND, target: Arc<WindowTarget>, text: Text) -> Result<Self> {
        Ok(Self {
            window: PickerWindow::create(owner, target, text, ViewKind::Details)?,
            text,
            group: Arc::from(""),
            records: Arc::from([]),
            epoch: 0,
        })
    }
    pub(crate) fn hwnd(&self) -> HWND {
        self.window.hwnd
    }
    pub(crate) fn active(&self) -> bool {
        self.window.visible()
    }
    pub(crate) fn group(&self) -> &Arc<str> {
        &self.group
    }
    pub(crate) fn open(
        &mut self,
        group: Arc<str>,
        records: Arc<[WindowRecord]>,
        selected: WindowIdentity,
        config: &Config,
        monitor: MonitorSnapshot,
    ) -> Result<()> {
        self.close();
        self.group = group;
        self.window.show(config, monitor)?;
        self.replace(records, Some(selected))?;
        self.window.focus()
    }
    pub(crate) fn refresh(&mut self, records: Arc<[WindowRecord]>) -> Result<()> {
        let selected = self
            .window
            .selected()
            .and_then(|index| self.records.get(index))
            .map(|record| record.identity);
        self.replace(records, selected)
    }
    fn replace(
        &mut self,
        records: Arc<[WindowRecord]>,
        previous: Option<WindowIdentity>,
    ) -> Result<()> {
        let selected = selection(&records, previous);
        let labels = records
            .iter()
            .map(|record| {
                crate::picker::label(if record.title.is_empty() {
                    self.text.untitled_window()
                } else {
                    &record.title
                })
            })
            .collect::<Vec<_>>();
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("details stage=epoch exhausted"))?;
        self.window.replace(&labels, selected, self.epoch)?;
        self.window
            .status(&self.text.details_status(records.len()))?;
        self.records = records;
        debug!("details stage=results count={}", self.records.len());
        Ok(())
    }
    pub(crate) fn reposition(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        self.window.position(config, monitor)
    }
    pub(crate) fn close(&mut self) {
        self.window.hide();
        self.records = Arc::from([]);
        self.group = Arc::from("");
    }
    pub(crate) fn poll(&mut self) -> Result<Option<DetailsAction>> {
        if !self.active() {
            return Ok(None);
        }
        let events = self.window.take_events();
        if events.flags & messages::CANCEL != 0 {
            return Ok(Some(DetailsAction::Cancel));
        }
        if events.flags & messages::BACK != 0 {
            return Ok(Some(DetailsAction::Back));
        }
        if events.flags & messages::RELAYOUT != 0 {
            self.window.layout()?;
        }
        Ok(events
            .accept
            .filter(|(epoch, _)| *epoch == self.epoch)
            .and_then(|(_, index)| self.records.get(index))
            .cloned()
            .map(DetailsAction::Activate))
    }
}

fn selection(records: &[WindowRecord], previous: Option<WindowIdentity>) -> usize {
    previous
        .and_then(|identity| {
            records
                .iter()
                .position(|record| record.identity == identity)
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_keeps_full_window_identity_and_rejects_pid_reuse() {
        let first = crate::app_identity::tests::record("app.exe", 1, 1, 1);
        let second = crate::app_identity::tests::record("app.exe", 2, 2, 1);
        assert_eq!(
            selection(&[second.clone(), first.clone()], Some(first.identity)),
            1
        );
        let mut stale = first.identity;
        stale.process.created += 1;
        assert_eq!(selection(&[second, first], Some(stale)), 0);
        assert_eq!(selection(&[], Some(stale)), 0);
        assert_eq!(crate::picker::label("标题\r\n窗口\t🚀"), "标题  窗口 🚀");
    }
}
