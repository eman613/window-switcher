use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
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
static DROPPED_METRICS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DROP_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Vec<MetricSample>> = Mutex::new(Vec::new());
static METRIC_TX: OnceLock<SyncSender<MetricSample>> = OnceLock::new();

const METRIC_QUEUE_CAPACITY: usize = 256;
const PENDING_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy)]
struct MetricSample {
    stage: &'static str,
    elapsed: Duration,
    sequence: Option<u64>,
}

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
            while let Ok(sample) = rx.recv() {
                write_sample_now(sample);
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
    for sample in pending {
        write_sample(sample);
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
    log_sample(MetricSample {
        stage,
        elapsed,
        sequence: None,
    });
}

fn log_sample(sample: MetricSample) {
    if !LOGGING_READY.load(Ordering::Acquire) {
        let mut pending = PENDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !LOGGING_READY.load(Ordering::Acquire) {
            if pending.len() < PENDING_QUEUE_CAPACITY {
                pending.push(sample);
            } else {
                record_metric_drop("pending metric queue full");
            }
            return;
        }
    }
    write_sample(sample);
}

pub(crate) fn record_keyboard_elapsed(stage: &'static str, sequence: u64, elapsed: Duration) {
    if !enabled() || sequence == 0 {
        return;
    }
    log_sample(MetricSample {
        stage,
        elapsed,
        sequence: Some(sequence),
    });
}

fn write_sample(sample: MetricSample) {
    if let Some(tx) = METRIC_TX.get() {
        match tx.try_send(sample) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => record_metric_drop("metric queue full"),
            Err(TrySendError::Disconnected(_)) => record_metric_drop("metric writer stopped"),
        }
    } else {
        write_sample_now(sample);
    }
}

fn write_sample_now(sample: MetricSample) {
    match sample.sequence {
        Some(sequence) => info!(
            "perf stage={} sequence={} elapsed_us={} dropped_metrics={}",
            sample.stage,
            sequence,
            sample.elapsed.as_micros(),
            DROPPED_METRICS.load(Ordering::Relaxed)
        ),
        None => info!(
            "perf stage={} elapsed_us={} dropped_metrics={}",
            sample.stage,
            sample.elapsed.as_micros(),
            DROPPED_METRICS.load(Ordering::Relaxed)
        ),
    }
}

fn record_metric_drop(reason: &str) {
    let dropped = DROPPED_METRICS.fetch_add(1, Ordering::Relaxed) + 1;
    if dropped == 1 && !DROP_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        warn!("performance metrics sample dropped: reason={reason} dropped_metrics={dropped}");
    }
}
