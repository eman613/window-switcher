use super::PreviewRequest;
use std::time::{Duration, Instant};

/// Only the current selection owns a deadline. No queued timers can outlive it.
#[derive(Default)]
pub(super) struct SelectionDelay {
    current: Option<PreviewRequest>,
    changed: Option<Instant>,
}

impl SelectionDelay {
    pub(super) fn select(&mut self, next: Option<PreviewRequest>, now: Instant) -> bool {
        let same_source = self.current.map(|value| (value.source, value.surface))
            == next.map(|value| (value.source, value.surface));
        if same_source {
            // Layout updates move an existing thumbnail; only a new source or
            // switching surface owns a new delay and DWM relationship.
            self.current = next;
            return false;
        }
        self.current = next;
        self.changed = next.map(|_| now);
        true
    }

    pub(super) fn ready(&self, now: Instant, delay: Duration) -> Option<PreviewRequest> {
        let changed = self.changed?;
        (now.saturating_duration_since(changed) >= delay)
            .then_some(self.current)
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_stable_current_selection_can_finish_its_delay() {
        let now = Instant::now();
        let first = super::super::fixture(1);
        let second = super::super::fixture(2);
        let mut delay = SelectionDelay::default();
        assert_eq!(delay.ready(now, Duration::ZERO), None);
        assert!(delay.select(Some(first), now));
        assert_eq!(delay.ready(now, Duration::ZERO), Some(first));
        let wait = Duration::from_millis(2000);
        assert_eq!(
            delay.ready(now + wait - Duration::from_nanos(1), wait),
            None
        );
        assert!(!delay.select(Some(first), now + wait));
        assert_eq!(delay.ready(now + wait, wait), Some(first));
        assert!(delay.select(Some(second), now + wait));
        assert_eq!(delay.ready(now + wait, wait), None);
        assert!(delay.select(None, now + wait));
        assert_eq!(delay.ready(now + wait * 10, wait), None);
    }

    #[test]
    fn identity_and_surface_restart_the_delay_but_geometry_keeps_the_lease() {
        let now = Instant::now();
        let first = super::super::fixture(1);
        let mut delay = SelectionDelay::default();
        delay.select(Some(first), now);
        let mut reused = first;
        reused.source.process.created += 1;
        assert!(delay.select(Some(reused), now));
        reused.surface += 1;
        assert!(delay.select(Some(reused), now));
        let ready = now + Duration::from_millis(250);
        reused.anchor.left += 1;
        assert!(!delay.select(Some(reused), ready));
        reused.monitor.dpi = 144;
        assert!(!delay.select(Some(reused), ready));
        assert_eq!(delay.ready(ready, Duration::from_millis(250)), Some(reused));
    }
}
