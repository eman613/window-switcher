//! CPU work counters, not a conversion from cycles to execution or wait time.
use std::time::Instant;
use windows::Win32::System::{
    Threading::GetCurrentThread, WindowsProgramming::QueryThreadCycleTime,
};

fn cycles() -> windows::core::Result<u64> {
    let mut value = 0;
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut value) }?;
    Ok(value)
}

pub(crate) fn measure<T>(enabled: bool, work: impl FnOnce() -> T) -> T {
    if !enabled {
        return work();
    }
    let before = cycles();
    let started = Instant::now();
    let result = work();
    let elapsed = started.elapsed();
    let after = cycles();
    match before.and_then(|start| after.map(|end| end.checked_sub(start))) {
        Ok(cpu_cycles) => debug!(
            "metrics event=thread_work stage=enumeration elapsed_us={} cpu_cycles={cpu_cycles:?}",
            elapsed.as_micros()
        ),
        Err(error) => debug!(
            "metrics event=thread_work stage=enumeration elapsed_us={} cpu_cycles=None error={:?}",
            elapsed.as_micros(),
            error.code()
        ),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_counter_is_monotonic_and_business_errors_are_preserved() {
        let before = cycles().unwrap();
        for enabled in [false, true] {
            let mut calls = 0;
            let result = measure(enabled, || {
                calls += 1;
                Err::<(), _>("cancelled")
            });
            assert_eq!(calls, 1);
            assert_eq!(result, Err("cancelled"));
        }
        assert!(cycles().unwrap() >= before);
    }
}
