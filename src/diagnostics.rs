use std::time::{Duration, Instant};

use windows::Win32::{
    Foundation::FILETIME,
    System::{
        SystemInformation::GetSystemTimeAsFileTime,
        Threading::{GetCurrentProcess, GetProcessTimes},
    },
};

use crate::config::Config;

pub(crate) fn sample_start(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

pub(crate) fn stage_elapsed(stage: &str, started: Option<Instant>) {
    if let Some(started) = started {
        info!(
            "metrics event=stage stage={stage} elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
}

pub(crate) struct Diagnostics {
    enabled: bool,
    main_enter: Instant,
    process_created: Option<u64>,
    interval: Duration,
    last_report: Instant,
    restart_started: Option<Instant>,
    ready: bool,
    restart_acks: u64,
    rollbacks: u64,
}

impl Diagnostics {
    pub(crate) fn new(configuration: &Config, main_enter: Instant) -> Self {
        Self {
            enabled: configuration.metrics_enabled,
            main_enter,
            process_created: configuration
                .metrics_enabled
                .then(process_created)
                .flatten(),
            interval: Duration::from_secs(configuration.metrics_interval_s.into()),
            last_report: Instant::now(),
            restart_started: None,
            ready: false,
            restart_acks: 0,
            rollbacks: 0,
        }
    }

    pub(crate) fn input_ready(&mut self) {
        if !self.enabled || self.ready {
            return;
        }
        self.ready = true;
        let elapsed = self
            .process_created
            .and_then(|created| ticks(unsafe { GetSystemTimeAsFileTime() }).checked_sub(created))
            .map(|ticks| ticks / 10);
        info!(
            "metrics event=input_ready pid={} main_elapsed_us={} process_elapsed_us={elapsed:?}",
            std::process::id(),
            self.main_enter.elapsed().as_micros()
        );
    }

    pub(crate) fn auxiliary_completed(&self, success: bool) {
        if self.enabled {
            info!(
                "metrics event=auxiliary_complete pid={} main_elapsed_us={} success={success}",
                std::process::id(),
                self.main_enter.elapsed().as_micros()
            );
        }
    }

    pub(crate) fn begin_restart(&mut self) {
        if self.enabled {
            self.restart_started = Some(Instant::now());
        }
    }

    pub(crate) fn restart_ack(&mut self) {
        if !self.enabled {
            return;
        }
        self.restart_acks += 1;
        info!(
            "metrics event=restart_active_ack elapsed_us={}",
            self.restart_started
                .map_or(0, |started| started.elapsed().as_micros())
        );
    }

    pub(crate) fn rollback(&mut self) {
        if self.enabled {
            self.rollbacks += 1;
        }
    }

    pub(crate) fn tick(&mut self) -> bool {
        if !self.enabled || self.last_report.elapsed() < self.interval {
            return false;
        }
        self.last_report = Instant::now();
        info!(
            "metrics event=summary restart_acks={} rollbacks={}",
            self.restart_acks, self.rollbacks
        );
        true
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new(&Config::default(), Instant::now())
    }
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn process_created() -> Option<u64> {
    let mut times = [FILETIME::default(); 4];
    let [created, exit, kernel, user] = &mut times;
    unsafe { GetProcessTimes(GetCurrentProcess(), created, exit, kernel, user) }.ok()?;
    Some(ticks(*created))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_metrics_have_no_sampling_or_periodic_output_work() {
        let mut diagnostics = Diagnostics::default();
        diagnostics.input_ready();
        diagnostics.begin_restart();
        diagnostics.restart_ack();
        diagnostics.rollback();
        assert!(!diagnostics.tick());
        assert!(diagnostics.process_created.is_none());
        assert!(diagnostics.restart_started.is_none());
        assert_eq!(diagnostics.restart_acks, 0);
    }
}
