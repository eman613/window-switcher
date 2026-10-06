use super::App;
use crate::{
    keyboard::state::SwitchKind, utils::window_identity::WindowIdentity,
    window_snapshot::filter::WindowFilter,
};
impl App {
    pub(super) fn request_window_close(
        &self,
        identity: WindowIdentity,
        executable: &str,
        kind: SwitchKind,
    ) -> &'static str {
        let filter = WindowFilter::from_config(&self.config, kind).with_scope(self.switching.scope);
        let posted = self.config.close_enable
            && self.owns_picker_foreground()
            && self.input.permits(self.input_session)
            && filter.allows_process(executable)
            && filter.allows(identity.hwnd()).is_some()
            && crate::window_actions::request_close(identity, &self.snapshots.lifetimes).is_ok();
        debug!("window-close stage=request posted={posted}");
        if posted {
            self.text.close_requested()
        } else {
            self.text.close_failed()
        }
    }
}
