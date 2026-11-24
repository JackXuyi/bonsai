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
    // T5 §5.6/A9 + plan-loop-supervisor：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("scheduler", move || {
        crate::failure::try_register(&app, "scheduler");
        let mut guard = crate::failure::PanicGuard::new("scheduler");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(POLL_SECONDS));
            crate::failure::try_heartbeat(&app, "scheduler");
            let res = guard.tick(|| tick(&app));
            match res {
                None if guard.stopped() => {
                    if crate::failure::on_loop_cap(&app, "scheduler", "consecutive panic cap") {
                        spawn(app.clone());
                    }
                    break;
                }
                Some(Ok(())) => {}
                Some(Err(e)) => log::warn!("consolidate tick failed: {e}"),
                None => {}
            }
        }
    });
}

fn tick(app: &AppHandle) -> Result<(), String> {
    let store = app.state::<MemoryStore>();
    let now_local = chrono::Local::now();
    let today = now_local.date_naive();

    // Short lock: the trigger decision needs two small reads only.
    let (last, age) = {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        (
            last_run_date(&conn).map_err(|e| e.to_string())?,
            hours_since_last_run(&conn),
        )
    };

    let Some(trigger) = decide_trigger(now_local.hour(), today, last, age) else {
        return Ok(());
    };

    // Phase 3 A5：统一 jobs 表闸（enabled + 连续失败 ≤3）；与 failure JobGuard 叠加。
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if !crate::memory::job::job_allows_run(&conn, "consolidate").map_err(|e| e.to_string())? {
            log::info!("consolidate: jobs table disabled or fail-capped, skipping");
            let _ = crate::memory::job::record_job_skipped(&conn, "consolidate");
            return Ok(());
        }
    }

    // T5 §5.5：退避门放在 decide_trigger **之后**——「该不该跑」（纯函数）
    // 与「能不能跑」（作业失败退避）是两件事。被退避挡住的夜归档下个
    // tick 再看，不消耗 trigger。
    let job_guard = crate::failure::JobGuard { name: "consolidate" };
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if !job_guard.should_run(&conn, now_local) {
            log::info!("consolidate: job backoff active, skipping this tick");
            return Ok(());
        }
    }

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
    let report = match consolidate::run_with_local_or_rules(&store, &opts) {
        Ok(r) => r,
        Err(e) => {
            // T5 §5.5/A8：失败要有名字——consolidate_runs 落一行 status='error'
            // + 原因（v17 error 列），退避状态写 settings（60s 起步，5 次当天停）。
            let detail = format!("consolidate run failed: {e}");
            {
                let conn = store.write_guard().map_err(|e2| e2.to_string())?;
                let st = record_consolidate_failure(
                    &conn, &job_guard, now_local, opts.trigger, &opts.window_start, &opts.window_end,
                    &detail,
                );
                let _ = crate::memory::job::record_job_failed(&conn, "consolidate");
                log::error!("consolidate 失败（fails={}，next_at={}）：{detail}", st.fails, st.next_at);
            }
            return Err(detail);
        }
    };

    // 跑成了 → 清退避，明天从零算起；jobs 表同步 ok。
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        job_guard.record_ok(&conn);
        let _ = crate::memory::job::record_job_ok(&conn, "consolidate");
    }

    // L2 摘要重写（plan-context §6.5，默认关）：巩固跑完接着做，失败/关闭都
    // 不影响上面那份 report。走本地 client——即便当前对话在云端，摘要是
    // 离线整理的活，不该为此把用户原文发出去。
    let digest_client = crate::llm::client_for(crate::llm::TaskLeg::Background);
    let digest = consolidate::digest_pass(&store, digest_client.as_ref());
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

    // plan-skill-auto-extract M1：巩固之后尝试自动成包；失败只 warn，不影响 consolidate。
    let skills_dir = app
        .try_state::<crate::mcp_admin::AgentState>()
        .map(|s| s.skills_dir.clone());
    if let Some(dir) = skills_dir {
        match crate::skill_auto::run_if_due(&store, &dir, false) {
            Ok(created) if !created.is_empty() => {
                for r in &created {
                    let _ = app.emit(
                        "skill:auto_extracted",
                        serde_json::json!({ "id": r.id, "name": r.name }),
                    );
                }
                log::info!("skill_auto: created {} pack(s)", created.len());
            }
            Ok(_) => {}
            Err(e) => log::warn!("skill_auto run_if_due failed: {e}"),
        }
    }

    Ok(())
}

/// T5 §5.5/A8：巩固失败要有名字——`consolidate_runs` 落一行 `status='error'`
/// + 原因（v17 `error` 列），退避状态写 settings。独立成函数以便单测。
fn record_consolidate_failure(
    conn: &rusqlite::Connection,
    guard: &crate::failure::JobGuard,
    now: chrono::DateTime<chrono::Local>,
    trigger: &str,
    window_start: &str,
    window_end: &str,
    detail: &str,
) -> crate::failure::JobState {
    let st = guard.record_fail(conn, now);
    let ts = super::clock::now_iso();
    conn.execute(
        "INSERT INTO consolidate_runs
             (started_at, finished_at, trigger, window_start, window_end,
              extracted, merged, contradicted, promoted, deleted, consumed,
              status, error)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, 0, 0, 0, 'error', ?6)",
        rusqlite::params![ts, ts, trigger, window_start, window_end, detail],
    )
    .ok();
    st
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

    #[test]
    fn a8_consolidate_failure_leaves_a_row_and_a_backoff() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::memory::db::SCHEMA).unwrap();
        let guard = crate::failure::JobGuard { name: "consolidate" };
        let now = chrono::Local::now();

        let st = record_consolidate_failure(
            &conn, &guard, now, "nightly", "1970-01-01T00:00:00Z", "2026-11-24T00:00:00Z",
            "consolidate run failed: model down",
        );
        assert_eq!(st.fails, 1);

        // 审计行：status='error' + 原因进 error 列。
        let (status, err): (String, String) = conn
            .query_row(
                "SELECT status, error FROM consolidate_runs WHERE status='error' ORDER BY id DESC LIMIT 1",
                [], |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "error");
        assert!(err.contains("model down"), "{err}");

        // 退避生效：30s 后 tick 不再跑（base 60s），到 next_at 放行。
        assert!(!guard.should_run(&conn, now + chrono::Duration::seconds(30)));
        let next = chrono::DateTime::parse_from_rfc3339(&st.next_at)
            .unwrap()
            .with_timezone(&chrono::Local);
        assert!(guard.should_run(&conn, next));
    }

    #[test]
    fn a8_five_failures_stop_the_day() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::memory::db::SCHEMA).unwrap();
        let guard = crate::failure::JobGuard { name: "consolidate" };
        let now = chrono::Local::now();
        for _ in 0..5 {
            record_consolidate_failure(
                &conn, &guard, now, "nightly", "1970", "2026", "boom",
            );
        }
        // fails=5 → next_at 推到明天 03:00：当天 tick 不再跑。
        assert!(
            !guard.should_run(&conn, now + chrono::Duration::hours(4)),
            "当天剩余时间都要被挡"
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM consolidate_runs WHERE status='error'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 5, "每次失败各留一行");
    }
