use crate::{
    config::Config,
    foreground::ForegroundStatus,
    keyboard::state::SwitchKind,
    mru::Mru,
    process_metadata::{ProcessMetadata, ProcessMetadataCache},
    utils::{com::ComApartment, window_identity::WindowIdentity},
    window_target::WindowTarget,
    worker::{self, Mailbox},
};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::Win32::Foundation::HWND;

pub(crate) mod filter;
pub(crate) mod lifetimes;
mod scan;
#[cfg(test)]
mod tests;

pub(crate) const WM_SNAPSHOT: u32 = 6010;

#[derive(Debug, Clone)]
pub(crate) struct WindowRecord {
    pub(crate) identity: WindowIdentity,
    pub(crate) process: ProcessMetadata,
    pub(crate) title: String,
    pub(crate) minimized: bool,
}

#[derive(Debug)]
pub(crate) struct WindowSnapshot {
    pub(crate) groups: IndexMap<Arc<str>, Vec<WindowRecord>>,
    pub(crate) revision: u64,
}

pub(crate) struct SnapshotService {
    mailbox: Arc<Mailbox<SwitchKind, Result<WindowSnapshot>>>,
    thread: Option<JoinHandle<()>>,
    pub(crate) lifetimes: Arc<lifetimes::WindowLifetimes>,
}

impl SnapshotService {
    pub(crate) fn start(
        config: &Config,
        is_admin: bool,
        foreground: Arc<ForegroundStatus>,
        lifetimes: Arc<lifetimes::WindowLifetimes>,
        target: Arc<WindowTarget>,
    ) -> Result<Self> {
        let mailbox = Mailbox::new(1);
        let shared = mailbox.clone();
        let configuration = config.clone();
        let registry = lifetimes.clone();
        let thread = thread::Builder::new()
            .name("window-snapshot".into())
            .spawn(move || {
                let _com = match ComApartment::sta() {
                    Ok(com) => com,
                    Err(error) => {
                        error!("snapshot stage=com error={error:#}");
                        shared.close();
                        target.try_post(WM_SNAPSHOT);
                        return;
                    }
                };
                let mut metadata = ProcessMetadataCache::new(&configuration);
                let mut foreground_version = 0;
                let mut mru = Mru::new(&configuration);
                while !shared.closed() && target.is_live() {
                    mru.observe_foreground(
                        foreground.resolve_pending(&mut metadata, &mut foreground_version),
                        &registry,
                    );
                    let Some((generation, kind)) = shared.receive(Duration::from_millis(20)) else {
                        continue;
                    };
                    let started = crate::diagnostics::sample_start(configuration.metrics_enabled);
                    let result = (|| {
                        let mut scan = scan::Scan::begin(
                            filter::WindowFilter::from_config(&configuration, kind),
                            is_admin,
                            registry.revision(),
                            target.window_id(),
                        )?;
                        while shared.current(generation) && target.is_live() {
                            if scan.step(
                                &mut metadata,
                                &registry,
                                Duration::from_millis(configuration.snapshot_budget_ms.into()),
                                |metadata| {
                                    mru.observe_foreground(
                                        foreground
                                            .resolve_pending(metadata, &mut foreground_version),
                                        &registry,
                                    );
                                    !shared.current(generation) || !target.is_live()
                                },
                            ) {
                                let mut snapshot = scan.finish()?;
                                mru.order(&mut snapshot, kind, &configuration);
                                return Ok(Some(snapshot));
                            }
                            thread::yield_now();
                        }
                        Ok(None)
                    })();
                    crate::diagnostics::stage_elapsed("enumeration", started);
                    if configuration.metrics_enabled {
                        info!(
                            "metrics event=metadata_cache entries={} hits={} misses={}",
                            metadata.len(),
                            metadata.hits,
                            metadata.misses
                        );
                    }
                    let output = match result {
                        Ok(Some(snapshot)) => Ok(snapshot),
                        Ok(None) => continue,
                        Err(error) => Err(error),
                    };
                    if shared.publish(generation, output) {
                        target.try_post(WM_SNAPSHOT);
                    }
                }
            })
            .context("snapshot stage=thread-create")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
            lifetimes,
        })
    }

    pub(crate) fn request(&self, kind: SwitchKind) -> u64 {
        self.mailbox.request(kind)
    }
    pub(crate) fn cancel(&self) {
        self.mailbox.cancel();
    }
    pub(crate) fn take(&self) -> Vec<(u64, Result<WindowSnapshot>)> {
        self.mailbox.take()
    }
    pub(crate) fn healthy(&self) -> bool {
        !self.mailbox.closed()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for SnapshotService {
    fn drop(&mut self) {
        self.mailbox.close();
        worker::retire(self.thread.take(), "snapshot");
    }
}

pub(crate) fn scan_for_tools(
    ignore_minimal: bool,
    only_current_desktop: bool,
    is_admin: bool,
) -> Result<IndexMap<String, Vec<(HWND, String)>>> {
    let _com = ComApartment::sta()?;
    let config = Config::default();
    let mut filter = filter::WindowFilter::from_config(&config, SwitchKind::Apps);
    filter.ignore_minimal = ignore_minimal;
    filter.only_current_desktop = only_current_desktop;
    let lifetimes = lifetimes::WindowLifetimes::default();
    let mut metadata = ProcessMetadataCache::new(&config);
    let mut scan = scan::Scan::begin(filter, is_admin, 0, 0)?;
    while !scan.step(&mut metadata, &lifetimes, Duration::from_millis(50), |_| {
        false
    }) {
        thread::yield_now();
    }
    Ok(scan
        .finish()?
        .groups
        .into_iter()
        .map(|(group, records)| {
            (
                group.to_string(),
                records
                    .into_iter()
                    .map(|record| (record.identity.hwnd(), record.title))
                    .collect(),
            )
        })
        .collect())
}
