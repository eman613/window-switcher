use crate::{
    app_identity::{AppIdentity, GroupResolver},
    config::Config,
    foreground::ForegroundStatus,
    keyboard::state::SwitchKind,
    monitor_scope::MonitorScope,
    mru::Mru,
    process_metadata::{ProcessMetadata, ProcessMetadataCache},
    utils::{com::ComApartment, window_identity::WindowIdentity},
    window_target::WindowTarget,
    worker::{self, Mailbox},
};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::{
    path::Path,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::Foundation::HWND;

mod enumeration;
pub(crate) mod filter;
pub(crate) mod lifetimes;
mod owners;
mod scan;
#[cfg(test)]
mod tests;
mod timings;

pub(crate) const WM_SNAPSHOT: u32 = 6010;

#[derive(Debug, Clone)]
pub(crate) struct WindowRecord {
    pub(crate) application: AppIdentity,
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
    mailbox: Arc<Mailbox<SnapshotRequest, SnapshotOutput>>,
    thread: Option<JoinHandle<()>>,
    pub(crate) lifetimes: Arc<lifetimes::WindowLifetimes>,
    metrics_enabled: bool,
}

struct SnapshotRequest {
    kind: SwitchKind,
    scope: MonitorScope,
    submitted: Option<Instant>,
}

struct SnapshotOutput {
    result: Result<WindowSnapshot>,
    completed: Option<Instant>,
}

impl SnapshotService {
    pub(crate) fn start(
        config: &Config,
        ini_dir: &Path,
        is_admin: bool,
        foreground: Arc<ForegroundStatus>,
        lifetimes: Arc<lifetimes::WindowLifetimes>,
        target: Arc<WindowTarget>,
    ) -> Result<Self> {
        let mailbox = Mailbox::<SnapshotRequest, SnapshotOutput>::new(1);
        let shared = mailbox.clone();
        let configuration = config.clone();
        let ini_dir = ini_dir.to_path_buf();
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
                let mut grouping = GroupResolver::new(&configuration, &ini_dir);
                let mut foreground_version = 0;
                let mut mru = Mru::new(&configuration);
                while !shared.closed() && target.is_live() {
                    mru.observe_foreground(
                        foreground.resolve_pending(&mut metadata, &mut foreground_version),
                        &registry,
                    );
                    let Some((generation, request)) = shared.receive(Duration::from_millis(20))
                    else {
                        continue;
                    };
                    let SnapshotRequest {
                        kind,
                        scope,
                        submitted,
                    } = request;
                    crate::diagnostics::stage_elapsed("snapshot-queue", submitted);
                    let started = crate::diagnostics::sample_start(configuration.metrics_enabled);
                    let result = crate::diagnostics::thread_work::measure(
                        log::log_enabled!(log::Level::Debug),
                        || {
                            let mut scan = scan::Scan::begin(
                                filter::WindowFilter::from_config(&configuration, kind)
                                    .with_scope(scope),
                                is_admin,
                                registry.revision(),
                                target.window_id(),
                            )?;
                            crate::diagnostics::stage_elapsed("snapshot-setup", started);
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
                                    let snapshot = scan.finish()?;
                                    let grouping_started = crate::diagnostics::sample_start(
                                        log::log_enabled!(log::Level::Debug),
                                    );
                                    let Some(mut snapshot) =
                                        grouping.regroup(snapshot, kind, || {
                                            shared.current(generation) && target.is_live()
                                        })?
                                    else {
                                        return Ok(None);
                                    };
                                    mru.order(&mut snapshot, kind, &configuration);
                                    crate::diagnostics::stage_elapsed(
                                        "snapshot-grouping",
                                        grouping_started,
                                    );
                                    return Ok(Some(snapshot));
                                }
                                thread::yield_now();
                            }
                            Ok(None)
                        },
                    );
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
                    let output = SnapshotOutput {
                        result: output,
                        completed: crate::diagnostics::sample_start(configuration.metrics_enabled),
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
            metrics_enabled: config.metrics_enabled,
        })
    }

    pub(crate) fn request(&self, kind: SwitchKind, scope: MonitorScope) -> u64 {
        self.mailbox.request(SnapshotRequest {
            kind,
            scope,
            submitted: crate::diagnostics::sample_start(self.metrics_enabled),
        })
    }
    pub(crate) fn cancel(&self) {
        self.mailbox.cancel();
    }
    pub(crate) fn take(&self) -> Vec<(u64, Result<WindowSnapshot>)> {
        self.mailbox
            .take()
            .into_iter()
            .map(|(generation, output)| {
                crate::diagnostics::stage_elapsed("snapshot-delivery", output.completed);
                (generation, output.result)
            })
            .collect()
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
