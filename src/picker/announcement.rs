use super::{messages::ViewState, ViewKind};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, LB_GETCURSEL};

impl ViewState {
    pub(super) fn announce(&self) {
        if self.kind != ViewKind::Search
            || !self.alive.get()
            || !self.visible.get()
            || (self.busy.get() && !self.failed.get())
            || self.composing.get()
        {
            return;
        }
        let Ok(announcer) = self.announcer.try_borrow() else {
            return;
        };
        let Some(announcer) = announcer.as_ref() else {
            return;
        };
        let Ok(visual) = self.visual.try_borrow() else {
            return;
        };
        let value = if self.failed.get() {
            self.text.search_failed_title().to_owned()
        } else if visual.rows.is_empty() {
            self.text.search_empty_title().to_owned()
        } else {
            let index = unsafe { SendMessageW(self.list.get(), LB_GETCURSEL, None, None) }.0;
            let Some(row) = usize::try_from(index)
                .ok()
                .and_then(|index| visual.rows.get(index))
            else {
                return;
            };
            row.accessible_label()
        };
        announcer.say(value);
    }

    pub(super) fn cancel_announcement(&self, reset_previous: bool) {
        if let Ok(announcer) = self.announcer.try_borrow() {
            if let Some(announcer) = announcer.as_ref() {
                if reset_previous {
                    announcer.cancel();
                } else {
                    announcer.suspend();
                }
            }
        }
    }
}
