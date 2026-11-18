//! Injectable clock (plan-memory.md §8): consolidation windows, time-decay
//! scoring and the eval metrics must run on a fixed timeline in tests.
//!
//! The override is **thread-local**: each test thread can pin its own instant
//! without racing the parallel test harness. Production threads simply never
//! set an override and get `chrono::Utc::now()`.

use std::cell::RefCell;

thread_local! {
    static OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Current instant as RFC3339 UTC, or the pinned override on this thread.
pub fn now_iso() -> String {
    OVERRIDE.with(|cell| {
        if let Some(fixed) = cell.borrow().as_ref() {
            return fixed.clone();
        }
        chrono::Utc::now().to_rfc3339()
    })
}

/// Pin the clock on the current thread (test only). Returns a guard that
/// clears the pin on drop.
#[cfg(test)]
pub fn pinned(iso: &str) -> ClockGuard {
    OVERRIDE.with(|cell| *cell.borrow_mut() = Some(iso.to_string()));
    ClockGuard
}

#[cfg(test)]
pub struct ClockGuard;

#[cfg(test)]
impl Drop for ClockGuard {
    fn drop(&mut self) {
        OVERRIDE.with(|cell| *cell.borrow_mut() = None);
    }
}
