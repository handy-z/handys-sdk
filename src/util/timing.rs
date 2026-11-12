use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};
use windows::core::PCWSTR;

thread_local! {
    // One timer per thread, never closed: it lives as long as the thread's waits do.
    static TIMER: Option<HANDLE> = unsafe {
        CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))
            .ok()
    };
}

/// Sleeps for `d` using a per-thread high-resolution waitable timer (falls back to a plain sleep).
/// Returns at once for zero.
pub fn precise_wait(d: Duration) {
    if d.is_zero() {
        return;
    }
    let Some(timer) = TIMER.with(|t| *t) else {
        return std::thread::sleep(d);
    };
    let due = -((d.as_nanos() / 100) as i64); // 100 ns units, negative = relative
    unsafe {
        if SetWaitableTimer(timer, &due, 0, None, None, false).is_ok() {
            WaitForSingleObject(timer, INFINITE);
        }
    }
}

/// Raises the system timer resolution to 1 ms while alive.
pub struct TimerResolution(());

impl TimerResolution {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        unsafe { timeBeginPeriod(1) };
        Self(())
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe { timeEndPeriod(1) };
    }
}

/// True once a condition has held without a break for longer than `after` (e.g. a read that should work keeps failing).
#[derive(Default)]
pub struct Sustained(Option<Instant>);

impl Sustained {
    pub fn update(&mut self, cond: bool, now: Instant, after: Duration) -> bool {
        if !cond {
            self.0 = None;
            return false;
        }
        now.duration_since(*self.0.get_or_insert(now)) > after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_sustained_fires_after_the_delay_and_resets() {
        let (t0, d) = (Instant::now(), Duration::from_secs(2));
        let mut s = Sustained::default();
        assert!(!s.update(true, t0, d));
        assert!(!s.update(true, t0 + d, d));
        assert!(s.update(true, t0 + d + Duration::from_millis(1), d));
        assert!(!s.update(false, t0 + Duration::from_secs(5), d));
        assert!(!s.update(true, t0 + Duration::from_secs(6), d));
    }

    #[test]
    fn timing_precise_wait_waits_at_least() {
        let t0 = Instant::now();
        precise_wait(Duration::from_millis(2));
        assert!(t0.elapsed() >= Duration::from_millis(2));
        let t0 = Instant::now();
        precise_wait(Duration::ZERO);
        assert!(t0.elapsed() < Duration::from_millis(1));
    }
}
