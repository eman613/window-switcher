use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::reload::ConfigCandidate;

struct Observation {
    candidate: ConfigCandidate,
    since: Instant,
    retry_at: Instant,
    attempts: u32,
    rejected: bool,
}

/// Observing and validating a version do not acknowledge it. Only a completed
/// input handoff changes `applied`; a failed version can therefore be retried.
pub(super) struct ChangeTracker {
    pub(super) observed: u64,
    pub(super) validated: u64,
    pub(super) applied_generation: u64,
    applied: Arc<[u8]>,
    current: Option<Observation>,
    in_flight: Option<ConfigCandidate>,
    delay: Duration,
    retry_delay: Duration,
    retry_limit: u32,
}

impl ChangeTracker {
    pub(super) fn new(
        bytes: Vec<u8>,
        delay: Duration,
        retry_delay: Duration,
        retry_limit: u32,
    ) -> Self {
        Self {
            observed: 0,
            validated: 0,
            applied_generation: 0,
            applied: bytes.into(),
            current: None,
            in_flight: None,
            delay,
            retry_delay,
            retry_limit,
        }
    }

    pub(super) fn observe(&mut self, bytes: Vec<u8>, now: Instant) {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.candidate.contents.as_ref() == bytes)
        {
            return;
        }
        if self.current.is_none() && self.applied.as_ref() == bytes {
            return;
        }
        self.observed += 1;
        self.current = Some(Observation {
            candidate: ConfigCandidate {
                generation: self.observed,
                contents: bytes.into(),
            },
            since: now,
            retry_at: now,
            attempts: 0,
            rejected: false,
        });
    }

    pub(super) fn unavailable(&mut self) {
        if self.current.take().is_some() {
            self.observed += 1;
        }
    }

    pub(super) fn next(&mut self, now: Instant) -> Option<ConfigCandidate> {
        if self.in_flight.is_some() {
            return None;
        }
        let current = self.current.as_mut()?;
        if current.candidate.contents.is_empty()
            || current.candidate.contents == self.applied
            || current.rejected
            || current.attempts > self.retry_limit
            || now.duration_since(current.since) < self.delay
            || now < current.retry_at
        {
            return None;
        }
        current.attempts += 1;
        let candidate = current.candidate.clone();
        self.in_flight = Some(candidate.clone());
        Some(candidate)
    }

    pub(super) fn validate(&mut self, generation: u64) {
        self.validated = generation;
    }

    pub(super) fn complete(
        &mut self,
        generation: u64,
        applied: bool,
        retryable: bool,
        now: Instant,
    ) {
        if !self
            .in_flight
            .as_ref()
            .is_some_and(|candidate| candidate.generation == generation)
        {
            return;
        }
        let candidate = self.in_flight.take().unwrap();
        if applied {
            self.applied = candidate.contents;
            self.applied_generation = generation;
        } else if let Some(current) = self
            .current
            .as_mut()
            .filter(|current| current.candidate.generation == generation)
        {
            current.rejected = !retryable;
            let multiplier = 1 << current.attempts.saturating_sub(1).min(5);
            current.retry_at = now + (self.retry_delay * multiplier).min(Duration::from_secs(30));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> ChangeTracker {
        ChangeTracker::new(
            b"old".to_vec(),
            Duration::from_secs(1),
            Duration::from_secs(2),
            2,
        )
    }

    #[test]
    fn repeated_saves_coalesce_and_only_active_ack_applies_a_version() {
        let now = Instant::now();
        let mut state = tracker();
        state.observe(b"first".to_vec(), now);
        state.observe(b"last".to_vec(), now + Duration::from_millis(500));
        assert!(state.next(now + Duration::from_secs(1)).is_none());
        let candidate = state.next(now + Duration::from_secs(2)).unwrap();
        state.validate(candidate.generation);
        assert_eq!(state.applied.as_ref(), b"old");
        assert!(state.next(now + Duration::from_secs(3)).is_none());
        state.complete(candidate.generation, true, false, now);
        assert_eq!(state.applied.as_ref(), b"last");
        assert!(state.next(now + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn same_bytes_retry_with_backoff_and_stop_at_the_budget() {
        let now = Instant::now();
        let mut state = tracker();
        state.observe(b"new".to_vec(), now);
        for second in [1, 3, 7] {
            let time = now + Duration::from_secs(second);
            let candidate = state.next(time).unwrap();
            state.complete(candidate.generation, false, true, time);
            assert!(state.next(time + Duration::from_millis(500)).is_none());
        }
        assert!(state.next(now + Duration::from_secs(60)).is_none());
        assert_eq!(state.applied.as_ref(), b"old");
        state.observe(b"newer".to_vec(), now + Duration::from_secs(61));
        assert!(state.next(now + Duration::from_secs(62)).is_some());
    }

    #[test]
    fn stale_completion_cannot_discard_a_newer_observation_or_ack_it() {
        let now = Instant::now();
        let mut state = tracker();
        state.observe(b"a".to_vec(), now);
        let a = state.next(now + Duration::from_secs(1)).unwrap();
        state.observe(b"b".to_vec(), now + Duration::from_secs(2));
        state.complete(a.generation, false, true, now + Duration::from_secs(3));
        let b = state.next(now + Duration::from_secs(3)).unwrap();
        assert_eq!(b.contents.as_ref(), b"b");
        state.complete(a.generation, true, false, now);
        assert_eq!(state.applied_generation, 0);
    }

    #[test]
    fn blank_missing_and_invalid_files_preserve_the_running_configuration() {
        let now = Instant::now();
        let mut state = tracker();
        state.observe(Vec::new(), now);
        assert!(state.next(now + Duration::from_secs(2)).is_none());
        state.observe(b"bad".to_vec(), now);
        let invalid = state.next(now + Duration::from_secs(2)).unwrap();
        state.complete(invalid.generation, false, false, now);
        assert!(state.next(now + Duration::from_secs(20)).is_none());
        state.unavailable();
        state.observe(b"old".to_vec(), now);
        assert!(state.next(now + Duration::from_secs(30)).is_none());
    }
}
