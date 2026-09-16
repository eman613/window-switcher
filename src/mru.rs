//! Session-only recency. No titles, paths, or history files are retained.
use crate::{
    config::{Config, SwitchOrder},
    keyboard::state::SwitchKind,
    process_metadata::ProcessIdentity,
    utils::window_identity::WindowIdentity,
    window_snapshot::{lifetimes::WindowLifetimes, WindowSnapshot},
};
use std::collections::{HashMap, VecDeque};
use windows::Win32::Foundation::HWND;

pub(crate) struct Mru {
    limit: usize,
    recent: VecDeque<WindowIdentity>,
    seeded: bool,
    revision: u64,
}

impl Mru {
    pub(crate) fn new(config: &Config) -> Self {
        let enabled = config.switch_apps_order == SwitchOrder::Mru
            || config.switch_windows_order == SwitchOrder::Mru;
        Self {
            limit: if enabled {
                config.mru_limit as usize
            } else {
                0
            },
            recent: VecDeque::new(),
            seeded: false,
            revision: 0,
        }
    }

    pub(crate) fn observe_foreground(
        &mut self,
        observed: Option<(HWND, ProcessIdentity)>,
        lifetimes: &WindowLifetimes,
    ) {
        if self.limit == 0 {
            return;
        }
        let revision = lifetimes.revision();
        if self.revision != revision {
            self.revision = revision;
            let before = self.recent.len();
            self.recent
                .retain(|identity| identity.has_current_lifetime(lifetimes));
            if before != self.recent.len() {
                debug!("mru stage=pruned count={}", before - self.recent.len());
            }
        }
        if let Some(identity) = observed
            .and_then(|(hwnd, process)| WindowIdentity::from_process(hwnd, process, lifetimes))
        {
            if self.observe(identity) {
                debug!("mru stage=foreground entries={}", self.recent.len());
            }
        }
    }

    fn observe(&mut self, identity: WindowIdentity) -> bool {
        if self.limit == 0 || self.recent.front() == Some(&identity) {
            return false;
        }
        // A reused HWND never inherits the old process/lifetime's rank.
        self.recent.retain(|old| old.window != identity.window);
        self.recent.push_front(identity);
        self.recent.truncate(self.limit);
        true
    }

    fn seed(&mut self, identities: impl IntoIterator<Item = WindowIdentity>) {
        if self.seeded || self.limit == 0 {
            return;
        }
        self.seeded = true;
        for identity in identities {
            if self.recent.len() == self.limit {
                break;
            }
            if !self.recent.contains(&identity) {
                self.recent.push_back(identity);
            }
        }
    }

    fn ranks(&self) -> HashMap<WindowIdentity, usize> {
        self.recent
            .iter()
            .copied()
            .enumerate()
            .map(|(rank, identity)| (identity, rank))
            .collect()
    }

    pub(crate) fn order(
        &mut self,
        snapshot: &mut WindowSnapshot,
        kind: SwitchKind,
        config: &Config,
    ) {
        let order = match kind {
            SwitchKind::Apps => config.switch_apps_order,
            SwitchKind::Windows => config.switch_windows_order,
            SwitchKind::Search => return,
        };
        if order == SwitchOrder::Existing {
            return;
        }
        self.seed(
            snapshot
                .groups
                .values()
                .flatten()
                .map(|record| record.identity),
        );
        let ranks = self.ranks();
        let rank = |identity: &WindowIdentity| ranks.get(identity).copied().unwrap_or(usize::MAX);
        for records in snapshot.groups.values_mut() {
            records.sort_by_key(|record| rank(&record.identity));
        }
        snapshot.groups.sort_by(|_, left, _, right| {
            left.first()
                .map(|record| rank(&record.identity))
                .cmp(&right.first().map(|record| rank(&record.identity)))
        });
        debug!(
            "mru stage=ordered groups={} entries={}",
            snapshot.groups.len(),
            self.recent.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(limit: u32) -> Mru {
        Mru::new(&Config {
            switch_apps_order: SwitchOrder::Mru,
            mru_limit: limit,
            ..Default::default()
        })
    }

    #[test]
    fn first_snapshot_follows_observed_foreground_then_seeds_stable_tail() {
        let ids: Vec<_> = (1..=5).map(WindowIdentity::fixture).collect();
        let mut history = tracked(4);
        history.observe(ids[2]);
        history.seed(ids.clone());
        assert_eq!(
            history.recent.iter().copied().collect::<Vec<_>>(),
            [ids[2], ids[0], ids[1], ids[3]]
        );
        history.seed([ids[4]]);
        assert_eq!(history.recent.len(), 4);
        history.observe(ids[4]);
        assert_eq!(
            history.recent.iter().copied().collect::<Vec<_>>(),
            [ids[4], ids[2], ids[0], ids[1]]
        );
        assert!(!history.observe(ids[4]));
    }

    #[test]
    fn reused_window_does_not_inherit_rank_and_history_is_bounded() {
        let original = WindowIdentity::fixture(1);
        let mut reused = original;
        reused.process.created += 1;
        let mut history = tracked(16);
        history.observe(original);
        assert!(!history.ranks().contains_key(&reused));
        history.observe(reused);
        assert!(!history.ranks().contains_key(&original));
        for window in 2..1000 {
            history.observe(WindowIdentity::fixture(window));
        }
        assert_eq!(history.recent.len(), 16);
        assert_eq!(history.recent.front().unwrap().window, 999);
        assert_eq!(history.recent.back().unwrap().window, 984);
    }

    #[test]
    fn existing_order_does_not_allocate_or_record_history() {
        let mut history = Mru::new(&Config::default());
        history.observe(WindowIdentity::fixture(1));
        history.seed([WindowIdentity::fixture(2)]);
        assert!(history.recent.is_empty());
        assert_eq!(history.recent.capacity(), 0);
    }

    #[test]
    fn lifecycle_events_evict_destroyed_and_reused_windows_without_process_queries() {
        let registry = WindowLifetimes::default();
        let mut history = tracked(16);
        history.observe(WindowIdentity::fixture(1));
        history.observe(WindowIdentity::fixture(2));
        registry.event(1, false);
        history.observe_foreground(None, &registry);
        assert_eq!(history.recent.len(), 2);
        registry.event(1, true);
        history.observe_foreground(None, &registry);
        assert_eq!(history.recent.len(), 1);
        assert_eq!(history.recent.front().unwrap().window, 2);
        registry.invalidate_all();
        history.observe_foreground(None, &registry);
        assert!(history.recent.is_empty());
    }

    #[test]
    fn app_and_window_order_are_independent_and_choose_the_recent_group_member() {
        use crate::{process_metadata::ProcessMetadata, window_snapshot::WindowRecord};
        use std::sync::Arc;
        let config = Config {
            switch_apps_order: SwitchOrder::Mru,
            ..Default::default()
        };
        let snapshot = || WindowSnapshot {
            revision: 1,
            groups: [("alpha", [1, 2]), ("beta", [3, 4])]
                .into_iter()
                .map(|(group, windows)| {
                    (
                        Arc::from(group),
                        windows
                            .into_iter()
                            .map(|window| {
                                let identity = WindowIdentity::fixture(window);
                                WindowRecord {
                                    identity,
                                    process: ProcessMetadata {
                                        identity: identity.process,
                                        path: Arc::from("fixture.exe"),
                                        executable: Arc::from("fixture.exe"),
                                        elevated: Some(false),
                                    },
                                    title: String::new(),
                                    minimized: false,
                                }
                            })
                            .collect(),
                    )
                })
                .collect(),
        };
        let mut history = Mru::new(&config);
        history.observe(WindowIdentity::fixture(2));
        history.observe(WindowIdentity::fixture(4));
        let mut apps = snapshot();
        history.order(&mut apps, SwitchKind::Apps, &config);
        assert_eq!(
            apps.groups.keys().map(AsRef::as_ref).collect::<Vec<&str>>(),
            ["beta", "alpha"]
        );
        assert_eq!(
            apps.groups
                .values()
                .flatten()
                .map(|record| record.identity.window)
                .collect::<Vec<_>>(),
            [4, 3, 2, 1]
        );
        let mut windows = snapshot();
        history.order(&mut windows, SwitchKind::Windows, &config);
        assert_eq!(
            windows
                .groups
                .values()
                .flatten()
                .map(|record| record.identity.window)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
    }
}
