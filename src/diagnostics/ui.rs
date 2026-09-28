//! Exclusive wall-time regions inside one panel show/focus call, including reentry.
use std::{
    cell::RefCell,
    marker::PhantomData,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::FILETIME,
    System::Threading::{GetCurrentThread, GetThreadTimes},
};

#[derive(Clone, Copy)]
enum Region {
    Outside,
    Callback,
    DefaultProc,
    Accessibility,
}

struct Trace {
    id: u64,
    last: Instant,
    region: Region,
    elapsed: [Duration; 4],
    callbacks: u64,
    cpu_started: Option<u64>,
    slow_default: (&'static str, Duration),
}

impl Trace {
    fn advance(&mut self, now: Instant) {
        self.elapsed[self.region as usize] += now.saturating_duration_since(self.last);
        self.last = now;
    }
    fn enter(&mut self, region: Region, now: Instant) -> Region {
        self.advance(now);
        let previous = self.region;
        self.region = region;
        if matches!(region, Region::Callback) {
            self.callbacks += 1;
        }
        previous
    }
}

struct ThreadTrace {
    next_id: u64,
    active: Option<Trace>,
}
thread_local! { static UI_TRACE: RefCell<ThreadTrace> = const { RefCell::new(ThreadTrace { next_id: 0, active: None }) }; }

pub(crate) struct UiCall {
    id: Option<u64>,
    phase: &'static str,
    _thread: PhantomData<*mut ()>,
}

impl UiCall {
    pub(crate) fn begin(phase: &'static str, enabled: bool) -> Self {
        let id = enabled
            .then(|| {
                UI_TRACE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    if state.active.is_some() {
                        return None;
                    }
                    state.next_id = state.next_id.wrapping_add(1);
                    let id = state.next_id;
                    let cpu_started = thread_cpu_ticks();
                    state.active = Some(Trace {
                        id,
                        last: Instant::now(),
                        region: Region::Outside,
                        elapsed: [Duration::ZERO; 4],
                        callbacks: 0,
                        cpu_started,
                        slow_default: ("none", Duration::ZERO),
                    });
                    Some(id)
                })
            })
            .flatten();
        Self {
            id,
            phase,
            _thread: PhantomData,
        }
    }
}

impl Drop for UiCall {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        let trace = UI_TRACE.with(|cell| {
            let mut state = cell.borrow_mut();
            if state.active.as_ref().is_none_or(|trace| trace.id != id) {
                return None;
            }
            let mut trace = state.active.take()?;
            trace.advance(Instant::now());
            Some(trace)
        });
        if let Some(trace) = trace {
            let cpu_us = trace
                .cpu_started
                .zip(thread_cpu_ticks())
                .and_then(|(start, end)| end.checked_sub(start))
                .map(|ticks| ticks / 10);
            let elapsed: Duration = trace.elapsed.iter().sum();
            info!("metrics event=ui_call phase={} elapsed_us={} outside_callbacks_us={} callback_us={} default_proc_us={} accessibility_us={} callbacks={} cpu_us={cpu_us:?} slow_default={} slow_default_us={}",
                self.phase, elapsed.as_micros(), trace.elapsed[0].as_micros(), trace.elapsed[1].as_micros(),
                trace.elapsed[2].as_micros(), trace.elapsed[3].as_micros(), trace.callbacks,
                trace.slow_default.0, trace.slow_default.1.as_micros());
        }
    }
}

pub(crate) struct UiRegion {
    previous: Option<(u64, Region, Instant)>,
    default_kind: Option<&'static str>,
    _thread: PhantomData<*mut ()>,
}

impl UiRegion {
    fn enter(region: Region, default_kind: Option<&'static str>) -> Self {
        let previous = UI_TRACE.with(|cell| {
            let mut state = cell.borrow_mut();
            let trace = state.active.as_mut()?;
            let now = Instant::now();
            Some((trace.id, trace.enter(region, now), now))
        });
        Self {
            previous,
            default_kind,
            _thread: PhantomData,
        }
    }
    pub(crate) fn callback() -> Self {
        Self::enter(Region::Callback, None)
    }
    pub(crate) fn accessibility() -> Self {
        Self::enter(Region::Accessibility, None)
    }
    pub(crate) fn default_proc(message: u32) -> Self {
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_ACTIVATE, WM_ACTIVATEAPP, WM_IME_NOTIFY, WM_IME_SETCONTEXT, WM_NCACTIVATE,
            WM_NCCALCSIZE, WM_SHOWWINDOW, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
        };
        let kind = match message {
            WM_ACTIVATE => "window-activation",
            WM_ACTIVATEAPP => "app-activation",
            WM_NCACTIVATE => "nonclient-activation",
            WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED | WM_NCCALCSIZE => "position",
            WM_SHOWWINDOW => "visibility",
            WM_IME_SETCONTEXT => "ime-context",
            WM_IME_NOTIFY => "ime-notify",
            _ => "other",
        };
        Self::enter(Region::DefaultProc, Some(kind))
    }
}

impl Drop for UiRegion {
    fn drop(&mut self) {
        let Some((id, previous, entered)) = self.previous else {
            return;
        };
        UI_TRACE.with(|cell| {
            let mut state = cell.borrow_mut();
            if let Some(trace) = state.active.as_mut().filter(|trace| trace.id == id) {
                let now = Instant::now();
                trace.advance(now);
                trace.region = previous;
                if let Some(kind) = self.default_kind {
                    let elapsed = now.saturating_duration_since(entered);
                    if elapsed > trace.slow_default.1 {
                        trace.slow_default = (kind, elapsed);
                    }
                }
            }
        });
    }
}

fn thread_cpu_ticks() -> Option<u64> {
    let mut times = [FILETIME::default(); 4];
    let [created, exit, kernel, user] = &mut times;
    unsafe { GetThreadTimes(GetCurrentThread(), created, exit, kernel, user) }.ok()?;
    super::ticks(*kernel).checked_add(super::ticks(*user))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_regions_partition_time_without_counting_native_reentry_twice() {
        let start = Instant::now();
        let mut trace = Trace {
            id: 1,
            last: start,
            region: Region::Outside,
            elapsed: [Duration::ZERO; 4],
            callbacks: 0,
            cpu_started: None,
            slow_default: ("none", Duration::ZERO),
        };
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        let outside = trace.enter(Region::Callback, at(2));
        let callback = trace.enter(Region::DefaultProc, at(3));
        let default_proc = trace.enter(Region::Callback, at(8));
        let nested = trace.enter(Region::Accessibility, at(9));
        trace.enter(nested, at(12));
        trace.enter(default_proc, at(13));
        trace.enter(callback, at(15));
        trace.enter(outside, at(16));
        trace.advance(at(20));
        assert_eq!(trace.elapsed, [6, 4, 7, 3].map(Duration::from_millis));
        assert_eq!(
            trace.elapsed.iter().sum::<Duration>(),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn disabled_and_nested_scopes_do_not_replace_an_active_measurement() {
        let disabled = UiCall::begin("disabled", false);
        assert!(disabled.id.is_none());
        assert!(UiRegion::callback().previous.is_none());
        let outer = UiCall::begin("outer", true);
        let nested = UiCall::begin("nested", true);
        assert!(nested.id.is_none());
        drop(nested);
        let region = UiRegion::callback();
        assert_eq!(region.previous.map(|value| value.0), outer.id);
        drop(region);
        drop(outer);
        assert!(UiRegion::callback().previous.is_none());
    }

    #[test]
    fn a_stale_region_cannot_modify_a_later_call() {
        let first = UiCall::begin("first", true);
        let region = UiRegion::callback();
        drop(first);
        let second = UiCall::begin("second", true);
        drop(region);
        UI_TRACE.with(|cell| {
            assert!(matches!(
                cell.borrow().active.as_ref().unwrap().region,
                Region::Outside
            ))
        });
        drop(second);
    }
}
