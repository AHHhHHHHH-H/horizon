//! Named timings within a frame, for finding what makes a frame slow:
//! `HORIZON_SPIKES=MS` prints the spans of every frame whose CPU time
//! exceeds MS milliseconds, and of every server tick (dedicated or
//! integrated) longer than MS. Costs one clock read per span when enabled
//! and a relaxed load otherwise.
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static SPANS: RefCell<Vec<(&'static str, f32)>> = const { RefCell::new(Vec::new()) };
}

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}
/// Run `f`, recording its duration under `name` when spans are enabled.
#[inline]
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Ordering::Relaxed) {
        return f();
    }
    let started = Instant::now();
    let out = f();
    let ms = started.elapsed().as_secs_f32() * 1000.0;
    SPANS.with(|s| s.borrow_mut().push((name, ms)));
    out
}
/// A span from `new` until dropped, for a block `time` can't wrap.
pub struct Span(Option<(&'static str, Instant)>);
impl Span {
    pub fn new(name: &'static str) -> Self {
        Self(
            ENABLED
                .load(Ordering::Relaxed)
                .then(|| (name, Instant::now())),
        )
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, started)) = self.0 {
            let ms = started.elapsed().as_secs_f32() * 1000.0;
            SPANS.with(|s| s.borrow_mut().push((name, ms)));
        }
    }
}
/// The CPU time this thread has used, where the platform tells it: a tick
/// with more wall time than CPU time waited (for a lock, or for a core on a
/// busy machine).
pub fn thread_cpu() -> Option<Duration> {
    #[cfg(unix)]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        Some(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
    }
    #[cfg(not(unix))]
    None
}
/// Whether spans are being recorded (`HORIZON_SPIKES`).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
/// Where this thread's spans are now, for `since`.
pub fn mark() -> usize {
    SPANS.with(|s| s.borrow().len())
}
/// The spans recorded since `mark`, summed by name, slowest first; they
/// stay for `take`. A part of the frame (the companions' hub tick) reports
/// its own slow ticks this way.
pub fn since(mark: usize) -> Vec<(&'static str, f32)> {
    SPANS.with(|s| sum(s.borrow().get(mark..).unwrap_or_default().to_vec()))
}
/// The frame's spans, summed by name, slowest first; clears them.
pub fn take() -> Vec<(&'static str, f32)> {
    sum(SPANS.with(|s| std::mem::take(&mut *s.borrow_mut())))
}
fn sum(spans: Vec<(&'static str, f32)>) -> Vec<(&'static str, f32)> {
    let mut sums: Vec<(&'static str, f32)> = Vec::new();
    for (name, ms) in spans {
        match sums.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 += ms,
            None => sums.push((name, ms)),
        }
    }
    sums.sort_by(|a, b| b.1.total_cmp(&a.1));
    sums
}

#[cfg(test)]
mod tests {
    /// The thread's CPU clock moves with work done on it, not with time
    /// spent asleep.
    #[cfg(unix)]
    #[test]
    fn thread_cpu_counts_work_not_sleep() {
        let start = super::thread_cpu().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let slept = super::thread_cpu().unwrap() - start;
        // Busy until the clock shows 10 ms of work (a loaded machine gives
        // the thread less than its wall time).
        let mut x = 0u64;
        let busy = std::time::Instant::now();
        let worked = loop {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
            let worked = super::thread_cpu().unwrap() - start - slept;
            if worked.as_millis() >= 10 || busy.elapsed().as_secs() >= 10 {
                break worked;
            }
        };
        assert!(slept.as_millis() < 25, "{slept:?} while asleep");
        assert!(worked.as_millis() >= 10, "{worked:?} while busy");
    }
}
