use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum QueryStage {
    Style,
    ProcessId,
    Cloak,
    Geometry,
    Dpi,
    Title,
    Metadata,
    Identity,
    OwnerMetadata,
    GroupKey,
    Interruption,
}

const STAGE_NAMES: [&str; QueryStage::Interruption as usize + 1] = [
    "style",
    "process-id",
    "cloak",
    "geometry",
    "dpi",
    "title",
    "metadata",
    "identity",
    "owner-metadata",
    "group-key",
    "interruption",
];

#[derive(Clone, Copy, Default)]
struct Measurement {
    calls: u32,
    total: Duration,
    maximum: Duration,
}

#[derive(Default)]
pub(super) struct QueryTimings {
    enabled: bool,
    measurements: [Measurement; STAGE_NAMES.len()],
}

impl QueryTimings {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    pub(super) fn measure<T>(&mut self, stage: QueryStage, query: impl FnOnce() -> T) -> T {
        let started = crate::diagnostics::sample_start(self.enabled);
        let result = query();
        if let Some(started) = started {
            let elapsed = started.elapsed();
            let measurement = &mut self.measurements[stage as usize];
            measurement.calls = measurement.calls.saturating_add(1);
            measurement.total = measurement.total.saturating_add(elapsed);
            measurement.maximum = measurement.maximum.max(elapsed);
        }
        result
    }

    pub(super) fn report(&self) {
        if !self.enabled {
            return;
        }
        for (stage, measurement) in STAGE_NAMES.iter().zip(&self.measurements) {
            debug!(
                "snapshot event=query_timing stage={stage} calls={} elapsed_us={} max_us={}",
                measurement.calls,
                measurement.total.as_micros(),
                measurement.maximum.as_micros()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_preserves_errors_and_disabled_queries_and_separates_stages() {
        let mut timings = QueryTimings::default();
        let mut calls = 0;
        let value = timings.measure(QueryStage::Metadata, || {
            calls += 1;
            Err::<(), _>("unavailable")
        });
        assert_eq!(value, Err("unavailable"));
        assert_eq!(calls, 1);
        assert_eq!(timings.measurements[QueryStage::Metadata as usize].calls, 0);
        timings.enabled = true;
        assert_eq!(timings.measure(QueryStage::Metadata, || None::<u32>), None);
        assert_eq!(timings.measure(QueryStage::Title, || 7), 7);
        assert_eq!(timings.measurements[QueryStage::Metadata as usize].calls, 1);
        assert_eq!(timings.measurements[QueryStage::Title as usize].calls, 1);
        assert_eq!(timings.measurements[QueryStage::Style as usize].calls, 0);
    }
}
