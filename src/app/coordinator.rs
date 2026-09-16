use crate::{
    icon_cache::IconKey, keyboard::state::SwitchKind, layout::MonitorSnapshot,
    utils::window_identity::WindowIdentity,
};
use anyhow::{ensure, Result};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) struct PendingSnapshot {
    pub(super) generation: u64,
    pub(super) kind: SwitchKind,
    pub(super) anchor: usize,
    pub(super) refresh: bool,
    pub(super) started: Instant,
}

#[derive(Clone, Copy)]
pub(super) struct QueuedCycle {
    pub(super) kind: SwitchKind,
    pub(super) reverse: bool,
    pub(super) received: Option<Instant>,
}

#[derive(Default)]
pub(super) struct SwitchCoordinator {
    pub(super) actions: VecDeque<QueuedCycle>,
    pub(super) pending: Option<PendingSnapshot>,
    pub(super) finishing: Option<SwitchKind>,
    pub(super) monitor: Option<MonitorSnapshot>,
    pub(super) scope: crate::monitor_scope::MonitorScope,
    pub(super) sticky: bool,
    pub(super) anchor: usize,
    pub(super) paint_dirty: bool,
    pub(super) return_focus: Option<WindowIdentity>,
    pub(super) icon_generation: u64,
    pub(super) icon_keys: Vec<IconKey>,
    pub(super) icon_pending: usize,
    pub(super) last_icons: Option<Instant>,
    pub(super) last_snapshot: Option<Instant>,
    pub(super) first_panel_started: Option<Instant>,
    pub(super) first_panel_done: bool,
    pub(super) selection_started: Option<Instant>,
}

impl SwitchCoordinator {
    pub(super) fn push(
        &mut self,
        kind: SwitchKind,
        reverse: bool,
        received: Option<Instant>,
    ) -> Result<()> {
        ensure!(
            self.actions.len() < 64,
            "input stage=async-actions capacity-exceeded"
        );
        self.actions.push_back(QueuedCycle {
            kind,
            reverse,
            received,
        });
        Ok(())
    }
    pub(super) fn can_finish(&self) -> bool {
        self.finishing.is_some() && self.pending.is_none() && self.actions.is_empty()
    }
    pub(super) fn timed_out(&self) -> bool {
        // A safety stop for an uninterruptible system query, separate from the
        // configured cooperative scan slice. No replacement thread is started.
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() >= Duration::from_secs(3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifier_release_waits_for_its_initial_scan_but_cancel_discards_it() {
        let mut state = SwitchCoordinator::default();
        state.push(SwitchKind::Apps, false, None).unwrap();
        state.pending = Some(PendingSnapshot {
            generation: 1,
            kind: SwitchKind::Apps,
            anchor: 10,
            refresh: false,
            started: Instant::now(),
        });
        state.finishing = Some(SwitchKind::Apps);
        assert!(!state.can_finish());
        state.pending = None;
        assert!(!state.can_finish());
        state.actions.pop_front();
        assert!(state.can_finish());
        state = SwitchCoordinator::default();
        assert!(state.pending.is_none() && state.finishing.is_none() && state.actions.is_empty());
    }
    #[test]
    fn asynchronous_input_and_request_age_are_bounded() {
        let mut state = SwitchCoordinator::default();
        for _ in 0..64 {
            state.push(SwitchKind::Windows, true, None).unwrap();
        }
        assert!(state.push(SwitchKind::Windows, true, None).is_err());
        state.pending = Some(PendingSnapshot {
            generation: 1,
            kind: SwitchKind::Windows,
            anchor: 10,
            refresh: false,
            started: Instant::now() - Duration::from_secs(3),
        });
        assert!(state.timed_out());
    }

    #[test]
    fn mixed_switch_actions_retain_their_original_receipt_times() {
        let mut state = SwitchCoordinator::default();
        let first = Instant::now();
        let second = first + Duration::from_millis(1);
        state.push(SwitchKind::Windows, false, Some(first)).unwrap();
        state.push(SwitchKind::Apps, true, Some(second)).unwrap();
        assert_eq!(state.actions.pop_front().unwrap().received, Some(first));
        let next = state.actions.pop_front().unwrap();
        assert_eq!(next.kind, SwitchKind::Apps);
        assert!(next.reverse);
        assert_eq!(next.received, Some(second));
    }
}
