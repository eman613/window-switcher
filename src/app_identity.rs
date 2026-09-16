//! Grouping identity is separate from the compatible icon lookup key.
mod profiles;
#[cfg(test)]
pub(crate) mod tests;

use crate::{
    config::{Config, Grouping},
    keyboard::state::SwitchKind,
    utils::browser,
    window_snapshot::WindowSnapshot,
};
use anyhow::{ensure, Result};
use indexmap::IndexMap;
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone)]
pub(crate) struct AppIdentity {
    pub(crate) key: Arc<str>,
    pub(crate) icon_key: Arc<str>,
    qualifier: Option<Arc<str>>,
}

impl AppIdentity {
    pub(crate) fn plain(icon_key: Arc<str>) -> Self {
        Self {
            key: icon_key.clone(),
            icon_key,
            qualifier: None,
        }
    }

    pub(crate) fn name(&self, base: &str) -> Arc<str> {
        self.qualifier.as_ref().map_or_else(
            || Arc::from(base),
            |qualifier| Arc::from(format!("{base} · {qualifier}")),
        )
    }
}

pub(crate) struct GroupResolver {
    policy: Grouping,
    profiles: Option<profiles::ProfileResolver>,
}

impl GroupResolver {
    pub(crate) fn new(config: &Config, ini_dir: &Path) -> Self {
        Self {
            policy: config.switch_apps_grouping,
            profiles: (config.switch_apps_grouping == Grouping::Profile)
                .then(|| profiles::ProfileResolver::new(config, ini_dir)),
        }
    }

    pub(crate) fn regroup(
        &mut self,
        snapshot: WindowSnapshot,
        kind: SwitchKind,
        current: impl Fn() -> bool,
    ) -> Result<Option<WindowSnapshot>> {
        if self.policy == Grouping::AppId || kind == SwitchKind::Windows {
            return Ok(current().then_some(snapshot));
        }
        let mut groups = IndexMap::<Arc<str>, Vec<_>>::new();
        let mut bytes = 0usize;
        for mut record in snapshot.groups.into_values().flatten() {
            if !current() {
                return Ok(None);
            }
            match self.policy {
                Grouping::Process => {
                    let identity = record.process.identity;
                    record.application.key =
                        format!("process:{}:{}", identity.pid, identity.created).into();
                    record.application.qualifier = Some(format!("PID {}", identity.pid).into());
                }
                Grouping::Profile => {
                    if let Some(profiles) = self.profiles.as_mut() {
                        if let Some((key, qualifier)) = browser::get_aumid(record.identity.hwnd())
                            .and_then(|aumid| profiles.resolve(&record.process.path, &aumid))
                        {
                            record.application.key = key;
                            record.application.qualifier = Some(qualifier);
                        }
                    }
                }
                Grouping::AppId => unreachable!(),
            }
            bytes = bytes.saturating_add(
                record.title.len()
                    + record.process.path.len()
                    + record.application.key.len()
                    + record.application.icon_key.len(),
            );
            ensure!(
                bytes <= 16 * 1024 * 1024,
                "grouping stage=snapshot text-budget exceeded"
            );
            groups
                .entry(record.application.key.clone())
                .or_default()
                .push(record);
        }
        debug!("grouping stage=resolved groups={}", groups.len());
        Ok(Some(WindowSnapshot {
            groups,
            revision: snapshot.revision,
        }))
    }
}
