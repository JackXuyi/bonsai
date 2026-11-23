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

/// Current instant in the machine's local timezone, or the pinned override
/// converted to local. Everything user-visible (env segment, day keys,
/// reminder windows) must go through this — raw `Local::now()` re-introduces
/// the date-drift flake that `proactive_spoke_today` already hit once.
pub fn now_local() -> chrono::DateTime<chrono::Local> {
    let fixed = OVERRIDE.with(|cell| cell.borrow().clone());
    match fixed {
        Some(iso) => chrono::DateTime::parse_from_rfc3339(&iso)
            .map(|t| t.with_timezone(&chrono::Local))
            .unwrap_or_else(|_| chrono::Local::now()),
        None => chrono::Local::now(),
    }
}

/// RFC3339 时间戳 → 本地「MM-DD」（摘要行前缀，plan-context §6.4）。
///
/// 为什么不用字符串切片：库里的 `ts` 是 UTC（`now_iso`），早上 8 点前的
/// 对话在 UTC 还是前一天——摘要上的日期会集体错一天。解析失败返回空串，
/// 调用方（`digest_line`）会退化成不带日期的行，不 panic。
pub fn day_mmdd(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&chrono::Local).format("%m-%d").to_string())
        .unwrap_or_default()
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
