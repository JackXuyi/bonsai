//! Phase 3 Autonomy · A5 — 统一常驻 job 表
//! （docs/zh/plan-autonomy-phase3/v1 §8；对齐 roadmap-agent-capabilities P3-5）。
//!
//! 执行器注册在 Rust 静态表；job 行不内嵌脚本。consolidate 仍调用
//! `consolidate::`，本模块只管调度元数据与失败计数。

use rusqlite::{params, Connection, OptionalExtension};

use crate::memory::clock;

pub const MAX_CONSECUTIVE_FAILS: i64 = 3;

#[derive(Debug, Clone)]
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

/// 种子：consolidate 夜间作业（幂等 upsert）。
pub fn seed_builtin_jobs(conn: &Connection) -> rusqlite::Result<()> {
    upsert_job(
        conn,
        "job-consolidate",
        "consolidate",
        "cron_like",
        "daily@03:00",
        true,
        "{}",
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
        |r| {
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
        },
    )
    .optional()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    #[test]
    fn seed_and_fail_cap() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        assert!(job_allows_run(&conn, "consolidate").unwrap());
        for _ in 0..MAX_CONSECUTIVE_FAILS {
            record_job_failed(&conn, "consolidate").unwrap();
        }
        assert!(!job_allows_run(&conn, "consolidate").unwrap());
        record_job_ok(&conn, "consolidate").unwrap();
        assert!(job_allows_run(&conn, "consolidate").unwrap());
    }

    #[test]
    fn upsert_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        seed_builtin_jobs(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM jobs WHERE name = 'consolidate'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1);
    }
}
