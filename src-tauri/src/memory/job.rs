//! P3-5 / Phase 3 A5 — 统一常驻 job 表
//! （docs/zh/plan-jobs/v1；docs/zh/plan-autonomy-phase3/v1 §8）。
//!
//! 执行器注册在 Rust；job 行不内嵌脚本。调度元数据与失败计数在此；
//! consolidate / calendar / report 执行体仍在各权威模块。

use chrono::{DateTime, Local, NaiveDate, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::memory::clock;

pub const MAX_CONSECUTIVE_FAILS: i64 = 3;

/// consolidate 与旧 `decide_trigger` 对齐的 catchup 阈值（小时）。
pub const CONSOLIDATE_CATCHUP_HOURS: f64 = 36.0;
pub const CONSOLIDATE_RUN_HOUR: u32 = 3;
pub const DIGEST_RUN_HOUR: u32 = 21;
pub const REMIND_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: String,
    pub name: String,
    pub schedule_kind: String,
    pub schedule_expr: String,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub consecutive_fails: i64,
    pub payload_json: String,
}

/// 种子三内置作业（幂等 upsert；不覆盖用户手动改过的 enabled——见 upsert 策略）。
pub fn seed_builtin_jobs(conn: &Connection) -> rusqlite::Result<()> {
    seed_job_keep_enabled(
        conn,
        "job-consolidate",
        "consolidate",
        "cron_like",
        "daily@03:00",
        "{}",
    )?;
    seed_job_keep_enabled(
        conn,
        "job-calendar-remind",
        "calendar_remind",
        "interval",
        "interval:60",
        "{}",
    )?;
    seed_job_keep_enabled(
        conn,
        "job-daily-digest",
        "daily_digest",
        "cron_like",
        "daily@21:00",
        "{}",
    )?;
    Ok(())
}

/// 首次插入默认 enabled=1；已存在则只刷新 schedule，**不**改 enabled（用户关闭保留）。
fn seed_job_keep_enabled(
    conn: &Connection,
    id: &str,
    name: &str,
    schedule_kind: &str,
    schedule_expr: &str,
    payload_json: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO jobs (id, name, schedule_kind, schedule_expr, enabled, payload_json)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)
         ON CONFLICT(name) DO UPDATE SET
           schedule_kind = excluded.schedule_kind,
           schedule_expr = excluded.schedule_expr,
           payload_json = excluded.payload_json",
        params![id, name, schedule_kind, schedule_expr, payload_json],
    )?;
    Ok(())
}

pub fn upsert_job(
    conn: &Connection,
    id: &str,
    name: &str,
    schedule_kind: &str,
    schedule_expr: &str,
    enabled: bool,
    payload_json: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO jobs (id, name, schedule_kind, schedule_expr, enabled, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET
           schedule_kind = excluded.schedule_kind,
           schedule_expr = excluded.schedule_expr,
           enabled = excluded.enabled,
           payload_json = excluded.payload_json",
        params![id, name, schedule_kind, schedule_expr, enabled as i64, payload_json],
    )?;
    Ok(())
}

pub fn get_job(conn: &Connection, name: &str) -> rusqlite::Result<Option<JobRow>> {
    conn.query_row(
        "SELECT id, name, schedule_kind, schedule_expr, enabled,
                last_run_at, last_status, consecutive_fails, payload_json
           FROM jobs WHERE name = ?1",
        params![name],
        map_job_row,
    )
    .optional()
}

pub fn list_jobs(conn: &Connection) -> rusqlite::Result<Vec<JobRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, schedule_kind, schedule_expr, enabled,
                last_run_at, last_status, consecutive_fails, payload_json
           FROM jobs ORDER BY name ASC",
    )?;
    let rows = stmt.query_map([], map_job_row)?;
    rows.collect()
}

fn map_job_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        name: r.get(1)?,
        schedule_kind: r.get(2)?,
        schedule_expr: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        last_run_at: r.get(5)?,
        last_status: r.get(6)?,
        consecutive_fails: r.get(7)?,
        payload_json: r.get(8)?,
    })
}

pub fn set_enabled(conn: &Connection, name: &str, enabled: bool) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE jobs SET enabled = ?2 WHERE name = ?1",
        params![name, enabled as i64],
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------- Tauri commands（P3-5 管理面）

#[tauri::command]
pub fn job_list(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<Vec<JobRow>, String> {
    store
        .with_write(list_jobs)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn job_set_enabled(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    name: String,
    enabled: bool,
) -> Result<bool, String> {
    if name.trim().is_empty() {
        return Err("job name required".into());
    }
    store
        .with_write(|conn| set_enabled(conn, &name, enabled))
        .map_err(|e| e.to_string())
}

/// 作业是否允许本 tick 跑（enabled + 失败未触顶）。
pub fn job_allows_run(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    match get_job(conn, name)? {
        None => Ok(true), // 表未种子时不挡既有路径
        Some(j) => Ok(j.enabled && j.consecutive_fails < MAX_CONSECUTIVE_FAILS),
    }
}

pub fn record_job_ok(conn: &Connection, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE jobs SET last_run_at = ?2, last_status = 'ok', consecutive_fails = 0
          WHERE name = ?1",
        params![name, clock::now_iso()],
    )?;
    Ok(())
}

pub fn record_job_failed(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "UPDATE jobs SET last_run_at = ?2, last_status = 'failed',
                consecutive_fails = consecutive_fails + 1
          WHERE name = ?1",
        params![name, clock::now_iso()],
    )?;
    Ok(get_job(conn, name)?.map(|j| j.consecutive_fails).unwrap_or(0))
}

pub fn record_job_skipped(conn: &Connection, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE jobs SET last_status = 'skipped' WHERE name = ?1",
        params![name],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- 到期判定（纯函数）

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueKind {
    Nightly,
    Catchup,
    Interval,
    Daily,
}

/// 解析 `last_run_at` → 本地日历日。
pub fn last_run_local_date(last_run_at: Option<&str>) -> Option<NaiveDate> {
    last_run_at
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|t| t.with_timezone(&Local).date_naive())
}

pub fn hours_since(last_run_at: Option<&str>, now: DateTime<Local>) -> Option<f64> {
    let last = DateTime::parse_from_rfc3339(last_run_at?).ok()?;
    Some((now.with_timezone(&chrono::Utc) - last.with_timezone(&chrono::Utc)).num_seconds() as f64 / 3600.0)
}

/// consolidate：对齐旧 `decide_trigger`（日界 + 36h catchup）。
pub fn consolidate_due(
    hour: u32,
    today: NaiveDate,
    last_run_date: Option<NaiveDate>,
    hours_since_last: Option<f64>,
) -> Option<DueKind> {
    if last_run_date == Some(today) {
        return None;
    }
    if let Some(hours) = hours_since_last {
        if hours > CONSOLIDATE_CATCHUP_HOURS {
            return Some(DueKind::Catchup);
        }
    }
    if hour >= CONSOLIDATE_RUN_HOUR {
        return Some(DueKind::Nightly);
    }
    None
}

/// `daily@HH:MM`：今日尚未成功/跳过跑过，且本地时 ≥ 指定时刻。
pub fn daily_at_due(
    expr: &str,
    now: DateTime<Local>,
    last_run_date: Option<NaiveDate>,
) -> bool {
    let Some((h, m)) = parse_daily_at(expr) else {
        return false;
    };
    if last_run_date == Some(now.date_naive()) {
        return false;
    }
    let minutes_now = now.hour() * 60 + now.minute();
    minutes_now >= h * 60 + m
}

fn parse_daily_at(expr: &str) -> Option<(u32, u32)> {
    let rest = expr.strip_prefix("daily@")?;
    let (h, m) = rest.split_once(':')?;
    Some((h.parse().ok()?, m.parse().ok()?))
}

/// `interval:N`：距上次 ≥ N 秒（或从未跑过）。
pub fn interval_due(expr: &str, now: DateTime<Local>, last_run_at: Option<&str>) -> bool {
    let secs: i64 = expr
        .strip_prefix("interval:")
        .and_then(|s| s.parse().ok())
        .unwrap_or(REMIND_INTERVAL_SECS);
    match last_run_at {
        None => true,
        Some(ts) => {
            let Ok(last) = DateTime::parse_from_rfc3339(ts) else {
                return true;
            };
            (now.with_timezone(&chrono::Utc) - last.with_timezone(&chrono::Utc)).num_seconds() >= secs
        }
    }
}

/// 通用：按 job 行判断是否到期（consolidate 用专用逻辑）。
pub fn is_due(job: &JobRow, now: DateTime<Local>) -> Option<DueKind> {
    if !job.enabled {
        return None;
    }
    match job.name.as_str() {
        "consolidate" => {
            let last_date = last_run_local_date(job.last_run_at.as_deref())
                // 兼容：巩固权威审计仍可在 scheduler 侧覆盖；此处用 jobs.last_run
                ;
            let age = hours_since(job.last_run_at.as_deref(), now);
            consolidate_due(now.hour(), now.date_naive(), last_date, age)
        }
        "calendar_remind" => {
            if interval_due(&job.schedule_expr, now, job.last_run_at.as_deref()) {
                Some(DueKind::Interval)
            } else {
                None
            }
        }
        "daily_digest" => {
            if daily_at_due(&job.schedule_expr, now, last_run_local_date(job.last_run_at.as_deref()))
            {
                Some(DueKind::Daily)
            } else {
                None
            }
        }
        _ => {
            // 未知 job：按 schedule_kind 尽力
            match job.schedule_kind.as_str() {
                "interval" if interval_due(&job.schedule_expr, now, job.last_run_at.as_deref()) => {
                    Some(DueKind::Interval)
                }
                "cron_like"
                    if daily_at_due(
                        &job.schedule_expr,
                        now,
                        last_run_local_date(job.last_run_at.as_deref()),
                    ) =>
                {
                    Some(DueKind::Daily)
                }
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;
    use chrono::TimeZone;

    #[test]
    fn seed_three_builtins() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        let names: Vec<String> = list_jobs(&conn)
            .unwrap()
            .into_iter()
            .map(|j| j.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "calendar_remind".to_string(),
                "consolidate".to_string(),
                "daily_digest".to_string()
            ]
        );
    }

    #[test]
    fn seed_preserves_disabled() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        set_enabled(&conn, "calendar_remind", false).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        assert!(!get_job(&conn, "calendar_remind").unwrap().unwrap().enabled);
    }

    #[test]
    fn fail_cap_blocks() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        for _ in 0..MAX_CONSECUTIVE_FAILS {
            record_job_failed(&conn, "consolidate").unwrap();
        }
        assert!(!job_allows_run(&conn, "consolidate").unwrap());
        record_job_ok(&conn, "consolidate").unwrap();
        assert!(job_allows_run(&conn, "consolidate").unwrap());
    }

    #[test]
    fn consolidate_due_matches_legacy() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(
            consolidate_due(3, d("2026-11-18"), Some(d("2026-11-17")), Some(15.0)),
            Some(DueKind::Nightly)
        );
        assert_eq!(
            consolidate_due(2, d("2026-11-18"), Some(d("2026-11-16")), Some(40.0)),
            Some(DueKind::Catchup)
        );
        assert_eq!(
            consolidate_due(4, d("2026-11-18"), Some(d("2026-11-18")), Some(1.0)),
            None
        );
    }

    #[test]
    fn daily_at_and_interval() {
        let now = Local.with_ymd_and_hms(2026, 11, 24, 21, 5, 0).unwrap();
        assert!(daily_at_due("daily@21:00", now, None));
        assert!(!daily_at_due("daily@21:00", now, Some(now.date_naive())));
        let early = Local.with_ymd_and_hms(2026, 11, 24, 20, 0, 0).unwrap();
        assert!(!daily_at_due("daily@21:00", early, None));

        assert!(interval_due("interval:60", now, None));
        let recent = (now - chrono::Duration::seconds(30)).to_rfc3339();
        assert!(!interval_due("interval:60", now, Some(&recent)));
        let old = (now - chrono::Duration::seconds(90)).to_rfc3339();
        assert!(interval_due("interval:60", now, Some(&old)));
    }

    #[test]
    fn is_due_respects_enabled() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        set_enabled(&conn, "daily_digest", false).unwrap();
        let job = get_job(&conn, "daily_digest").unwrap().unwrap();
        let now = Local.with_ymd_and_hms(2026, 11, 24, 22, 0, 0).unwrap();
        assert!(is_due(&job, now).is_none());
    }
}
