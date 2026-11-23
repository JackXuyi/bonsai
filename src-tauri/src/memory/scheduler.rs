//! Nightly scheduler (plan-memory.md §6).
//!
//! One 60s polling thread — no extra scheduling crate for MVP. Trigger logic
//! lives in `decide_trigger`, a pure function, so the scheduling rules are
//! unit-testable without Tauri or real time.
//!
//! Rules (local time):
//! - 03:00 nightly run, once per local day;
//! - on startup: if today ≥ 03:00 and today hasn't run → catchup;
//! - on startup: last run older than 36h → catchup (regardless of hour).
//!
//! Window: `[last window_end, today 00:00 local)` in UTC RFC3339. Unconsumed
//! events outside the window are never touched (plan §5.4); anything skipped
//! stays pending for the next pass.

use chrono::Timelike;
use tauri::{AppHandle, Manager};

use super::consolidate::{self, Options};
use super::MemoryStore;

const RUN_HOUR: u32 = 3;
const POLL_SECONDS: u64 = 60;
const CATCHUP_AFTER_HOURS: i64 = 36;

#[derive(Debug, PartialEq, Eq)]
pub enum Trigger {
    Nightly,
    Catchup,
}

/// Pure trigger decision. `last_run_date` = local date of the last done run
/// (nightly or catchup); `hours_since_last` = age of that run.
pub fn decide_trigger(
    hour: u32,
    today: chrono::NaiveDate,
    last_run_date: Option<chrono::NaiveDate>,
    hours_since_last: Option<f64>,
) -> Option<Trigger> {
    if last_run_date == Some(today) {
        return None; // already ran today — 防重
    }
    if let Some(hours) = hours_since_last {
        if hours > CATCHUP_AFTER_HOURS as f64 {
            return Some(Trigger::Catchup);
        }
    }
    if hour >= RUN_HOUR {
        return Some(Trigger::Nightly);
    }
    None
}

/// RFC3339 UTC instant of "local midnight that started today".
fn local_midnight_utc_iso() -> String {
    let now_local = chrono::Local::now();
    let midnight_local = now_local.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let offset = *now_local.offset();
    midnight_local
        .and_local_timezone(offset)
        .single()
        .map(|t| t.with_timezone(&chrono::Utc).to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
}

/// Local date of the last done consolidation run (from the audit table).
fn last_run_date(conn: &rusqlite::Connection) -> rusqlite::Result<Option<chrono::NaiveDate>> {
    let last: Option<String> = conn
        .query_row(
            "SELECT MAX(finished_at) FROM consolidate_runs WHERE status = 'done'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(None);
    Ok(last
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
        .map(|t| t.with_timezone(&chrono::Local).date_naive()))
}

fn hours_since_last_run(conn: &rusqlite::Connection) -> Option<f64> {
    let last: Option<String> = conn
        .query_row(
            "SELECT MAX(finished_at) FROM consolidate_runs WHERE status = 'done'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(None);
    let last = chrono::DateTime::parse_from_rfc3339(&last?).ok()?;
    Some((chrono::Utc::now() - last.with_timezone(&chrono::Utc)).num_seconds() as f64 / 3600.0)
}

/// Spawn the polling thread. Never blocks startup; errors are logged, the
/// loop keeps running (a bad night must not kill the companion).
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(POLL_SECONDS));
        if let Err(e) = tick(&app) {
            log::warn!("consolidate tick failed: {e}");
        }
    });
}

fn tick(app: &AppHandle) -> Result<(), String> {
    let store = app.state::<MemoryStore>();
    let now_local = chrono::Local::now();
    let today = now_local.date_naive();

    // Short lock: the trigger decision needs two small reads only.
    let (last, age) = {
        let conn = store.0.lock().map_err(|e| e.to_string())?;
        (
            last_run_date(&conn).map_err(|e| e.to_string())?,
            hours_since_last_run(&conn),
        )
    };

    let Some(trigger) = decide_trigger(now_local.hour(), today, last, age) else {
        return Ok(());
    };

    // Window: everything before today's local midnight. Unconsumed events
    // before the last window_end stay pending and get swept here too, because
    // unconsumed_events() keys off `consumed`, not just the window bounds.
    let window_start = last
        .and_then(|d| {
            d.and_hms_opt(0, 0, 0)
                .map(|t| {
                    t.and_local_timezone(*now_local.offset())
                        .single()
                        .map(|t| t.with_timezone(&chrono::Utc).to_rfc3339())
                })
                .flatten()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let window_end = local_midnight_utc_iso();

    let opts = Options {
        trigger: match trigger {
            Trigger::Nightly => "nightly",
            Trigger::Catchup => "catchup",
        },
        window_start: &window_start,
        window_end: &window_end,
    };

    // Store-backed entry: the extraction's LLM round runs with ZERO locks
    // (2026-11-20 lock-narrowing) — no guard is held here, so a catchup
    // consolidation can never freeze chat turns racing for the store.
    let report = consolidate::run_with_local_or_rules(&store, &opts)
        .map_err(|e| format!("consolidate run failed: {e}"))?;

    // L2 摘要重写（plan-context §6.5，默认关）：巩固跑完接着做，失败/关闭都
    // 不影响上面那份 report。走本地 client——即便当前对话在云端，摘要是
    // 离线整理的活，不该为此把用户原文发出去。
    let digest = consolidate::digest_pass(&store, &crate::llm::OllamaClient::from_env());
    if digest.enabled {
        log::info!(
            "digest pass: rewritten {} skipped {}",
            digest.rewritten,
            digest.skipped
        );
    }

    // Growth-narrative hook: the frontend can surface "it thought about
    // yesterday" from this event (plan §6).
    use tauri::Emitter;
    app.emit("consolidate:done", &report)
        .map_err(|e| format!("emit consolidate:done failed: {e}"))?;
    log::info!(
        "consolidate ({}) done: extracted {} merged {} contradicted {} promoted {} deleted {}",
        report.trigger,
        report.extracted,
        report.merged,
        report.contradicted,
        report.promoted,
        report.deleted
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn nightly_fires_at_and_after_run_hour() {
        assert_eq!(decide_trigger(3, d("2026-11-18"), Some(d("2026-11-17")), Some(15.0)), Some(Trigger::Nightly));
        assert_eq!(decide_trigger(5, d("2026-11-18"), Some(d("2026-11-17")), Some(26.0)), Some(Trigger::Nightly));
        assert_eq!(decide_trigger(2, d("2026-11-18"), Some(d("2026-11-17")), Some(20.0)), None);
    }

    #[test]
    fn never_twice_per_day() {
        assert_eq!(decide_trigger(4, d("2026-11-18"), Some(d("2026-11-18")), Some(1.0)), None);
        assert_eq!(decide_trigger(23, d("2026-11-18"), Some(d("2026-11-18")), Some(20.0)), None);
    }

    #[test]
    fn first_ever_run_fires_nightly() {
        assert_eq!(decide_trigger(10, d("2026-11-18"), None, None), Some(Trigger::Nightly));
    }

    #[test]
    fn stale_last_run_catchups_even_before_run_hour() {
        // last run 2026-11-16, now 2026-11-18 02:00, age 40h → catchup wins.
        assert_eq!(decide_trigger(2, d("2026-11-18"), Some(d("2026-11-16")), Some(40.0)), Some(Trigger::Catchup));
    }

    #[test]
    fn yesterday_run_aged_36h_plus_catchups() {
        assert_eq!(decide_trigger(1, d("2026-11-18"), Some(d("2026-11-17")), Some(37.0)), Some(Trigger::Catchup));
    }
}
