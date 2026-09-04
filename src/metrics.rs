use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

/// Lightweight opt-in timing for the hot paths.
///
/// Timing is disabled unless `WINDOW_SWITCHER_PERF` is set to a truthy value
/// (`1`, `true`, `yes`, or `on`) before the process starts.  Keeping the switch
/// outside the normal configuration avoids adding work to the default path and
/// still makes measurements reproducible from the PowerShell sampler.
static ENABLED: OnceLock<bool> = OnceLock::new();
static LOGGING_READY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());
static METRIC_TX: OnceLock<SyncSender<(&'static str, Duration)>> = OnceLock::new();

const METRIC_QUEUE_CAPACITY: usize = 256;

pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("WINDOW_SWITCHER_PERF")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

pub(crate) fn mark_logging_ready() {
    if !enabled() {
        return;
    }
    let (tx, rx) = mpsc::sync_channel(METRIC_QUEUE_CAPACITY);
    match thread::Builder::new()
        .name("window-switcher-metrics".to_string())
        .spawn(move || {
            while let Ok((stage, elapsed)) = rx.recv() {
                write_duration_now(stage, elapsed);
            }
        }) {
        Ok(_) => {
            let _ = METRIC_TX.set(tx);
        }
        Err(err) => warn!("failed to start performance metrics writer: {err}"),
    }
    let pending = {
        let mut pending = PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LOGGING_READY.store(true, Ordering::Release);
        pending.drain(..).collect::<Vec<_>>()
    };
    for (stage, elapsed) in pending {
        write_duration(stage, elapsed);
    }
}

pub(crate) struct StageTimer {
    stage: &'static str,
    started: Option<Instant>,
}

impl StageTimer {
    pub(crate) fn new(stage: &'static str) -> Self {
        Self {
            stage,
            started: enabled().then(Instant::now),
        }
    }

    pub(crate) fn finish(self) {
        // Dropping the timer records the measurement.  This method exists for
        // call sites that want to make the end of a phase explicit.
        drop(self);
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let elapsed = started.elapsed();
        log_duration(self.stage, elapsed);
    }
}

fn log_duration(stage: &'static str, elapsed: Duration) {
    if !LOGGING_READY.load(Ordering::Acquire) {
        let mut pending = PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !LOGGING_READY.load(Ordering::Acquire) {
            pending.push((stage, elapsed));
            return;
        }
    }
    write_duration(stage, elapsed);
}

fn write_duration(stage: &'static str, elapsed: Duration) {
    if let Some(tx) = METRIC_TX.get() {
        let _ = tx.try_send((stage, elapsed));
    } else {
        write_duration_now(stage, elapsed);
    }
}

fn write_duration_now(stage: &'static str, elapsed: Duration) {
    info!("perf stage={} elapsed_us={}", stage, elapsed.as_micros());
}
