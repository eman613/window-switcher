//! Test-only setup for session regressions without stealing desktop focus.
use super::*;

impl PickerWindow {
    pub(crate) fn fixture_rows(&self, rows: Vec<PickerRow>, epoch: u64) -> Result<HWND> {
        self.state().visible.set(true);
        self.replace_rows(rows, 0, epoch)?;
        self.state().flags.set(0);
        Ok(self.controls().edit)
    }
}
