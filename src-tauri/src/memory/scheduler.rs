//! P3-5 统一常驻调度（docs/zh/plan-jobs/v1）。
//!
//! 一条 60s 轮询线程驱动 `jobs` 表内置作业：consolidate / calendar_remind /
//! daily_digest。执行体仍在各权威模块；本文件只做到期判定与分发。

use chrono::Timelike;
use tauri::{AppHandle, Emitter, Manager};

use super::consolidate::{self, Options};
use super::job::{self, DueKind};
use super::MemoryStore;

const POLL_SECONDS: u64 = 60;

/// Spawn the polling thread. Never blocks startup; errors are logged, the
/// loop keeps running (a bad night must not kill the companion).
pub fn spawn(app: AppHandle) {
    crate::failure::spawn_guarded("scheduler", move || {
        crate::failure::try_register(&app, "scheduler");
        let mut guard = crate::failure::PanicGuard::new("scheduler");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(POLL_SECONDS));
            crate::failure::try_heartbeat(&app, "scheduler");
            let res = guard.tick(|| tick_all(&app));
            match res {
                None if guard.stopped() => {
                    if crate::failure::on_loop_cap(&app, "scheduler", "consecutive panic cap") {
                        spawn(app.clone());
                    }
                    break;
                }
                Some(Ok(())) => {}
                Some(Err(e)) => log::warn!("jobs tick failed: {e}"),
                None => {}
            }
        }
    });
}

fn tick_all(app: &AppHandle) -> Result<(), String> {
    let store = app.state::<MemoryStore>();
    let now_local = chrono::Local::now();

    let jobs = {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        job::list_jobs(&conn).map_err(|e| e.to_string())?
    };

    for j in jobs {
        if !j.enabled {
            continue;
        }
        let allows = {
            let conn = store.write_guard().map_err(|e| e.to_string())?;
            job::job_allows_run(&conn, &j.name).map_err(|e| e.to_string())?
        };
        if !allows {
            let conn = store.write_guard().map_err(|e| e.to_string())?;
            let _ = job::record_job_skipped(&conn, &j.name);
            continue;
        }

        match j.name.as_str() {
            "consolidate" => {
                if let Some(kind) = consolidate_due_from_audit(&store, now_local)? {
                    run_consolidate(app, &store, now_local, kind)?;
                }
            }
            "calendar_remind" => {
                if job::is_due(&j, now_local).is_some() {
                    run_calendar_remind(app, &store)?;
                }
            }
            "daily_digest" => {
                if job::is_due(&j, now_local).is_some() {
                    run_daily_digest(app, &store, now_local)?;
                }
            }
            other => log::debug!("jobs: unknown name `{other}`, skip"),
        }
    }
    Ok(())
}

/// consolidate 到期仍以 `consolidate_runs` 为权威日界（避免 jobs 种子后双跑）。
fn consolidate_due_from_audit(
    store: &MemoryStore,
    now_local: chrono::DateTime<chrono::Local>,
) -> Result<Option<DueKind>, String> {
    let conn = store.write_guard().map_err(|e| e.to_string())?;
    let last = last_consolidate_run_date(&conn).map_err(|e| e.to_string())?;
    let age = hours_since_last_consolidate(&conn);
    Ok(job::consolidate_due(
        now_local.hour(),
        now_local.date_naive(),
        last,
        age,
    ))
}

fn last_consolidate_run_date(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Option<chrono::NaiveDate>> {
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

fn hours_since_last_consolidate(conn: &rusqlite::Connection) -> Option<f64> {
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

fn run_consolidate(
    app: &AppHandle,
    store: &MemoryStore,
    now_local: chrono::DateTime<chrono::Local>,
    kind: DueKind,
) -> Result<(), String> {
    let job_guard = crate::failure::JobGuard { name: "consolidate" };
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if !job_guard.should_run(&conn, now_local) {
            log::info!("consolidate: job backoff active, skipping this tick");
            return Ok(());
        }
    }

    let last = {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        last_consolidate_run_date(&conn).map_err(|e| e.to_string())?
    };
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
        trigger: match kind {
            DueKind::Catchup => "catchup",
            _ => "nightly",
        },
        window_start: &window_start,
        window_end: &window_end,
    };

    let report = match consolidate::run_with_local_or_rules(store, &opts) {
        Ok(r) => r,
        Err(e) => {
            let detail = format!("consolidate run failed: {e}");
            {
                let conn = store.write_guard().map_err(|e2| e2.to_string())?;
                let st = record_consolidate_failure(
                    &conn,
                    &job_guard,
                    now_local,
                    opts.trigger,
                    &opts.window_start,
                    &opts.window_end,
                    &detail,
                );
                let _ = job::record_job_failed(&conn, "consolidate");
                log::error!(
                    "consolidate 失败（fails={}，next_at={}）：{detail}",
                    st.fails,
                    st.next_at
                );
            }
            return Err(detail);
        }
    };

    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        job_guard.record_ok(&conn);
        let _ = job::record_job_ok(&conn, "consolidate");
    }

    // L2 摘要重写仍挂在巩固成功后（默认关）；不算独立 job 日界。
    let digest_client = crate::llm::client_for(crate::llm::TaskLeg::Background);
    let digest = consolidate::digest_pass(store, digest_client.as_ref());
    if digest.enabled {
        log::info!(
            "digest pass: rewritten {} skipped {}",
            digest.rewritten,
            digest.skipped
        );
    }

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

    let skills_dir = app
        .try_state::<crate::mcp_admin::AgentState>()
        .map(|s| s.skills_dir.clone());
    if let Some(dir) = skills_dir {
        match crate::skill_auto::run_if_due(store, &dir, false) {
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

fn run_calendar_remind(app: &AppHandle, store: &MemoryStore) -> Result<(), String> {
    let Some(state) = app.try_state::<crate::calendar::CalendarState>() else {
        return Ok(());
    };
    let now = chrono::Local::now();
    let result = {
        let mut conn = store.write_guard().map_err(|e| e.to_string())?;
        crate::calendar::tick(&mut conn, state.source(), now)
            .map_err(|e| format!("calendar tick: {}", e.code()))?
    };
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        let _ = job::record_job_ok(&conn, "calendar_remind");
    }
    if let Some(r) = result {
        let _ = app.emit("calendar:remind", r);
    }
    Ok(())
}

fn run_daily_digest(
    app: &AppHandle,
    store: &MemoryStore,
    now_local: chrono::DateTime<chrono::Local>,
) -> Result<(), String> {
    let day = now_local.date_naive();
    let client = crate::llm::client_for(crate::llm::TaskLeg::Background);
    let cal = app.try_state::<crate::calendar::CalendarState>();

    let markdown = {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        let agenda = match &cal {
            Some(state) if crate::calendar::is_enabled(&conn) => {
                let src = state.source();
                if src.access().can_read() {
                    crate::calendar::agenda_for_day(&conn, day + chrono::Duration::days(1))
                        .ok()
                        .map(|items| crate::report::TomorrowAgenda { items, note: None })
                } else {
                    Some(crate::report::TomorrowAgenda {
                        items: Vec::new(),
                        note: Some("（日历未授权，看不到明天安排）".into()),
                    })
                }
            }
            _ => None,
        };
        crate::report::build_with_calendar(&conn, Some(client.as_ref()), day, agenda.as_ref())
    };

    let empty = markdown.contains("没有记录到任何活动") || markdown.trim().is_empty();
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if empty {
            // 推进 last_run，避免同日空转；status 先 skipped 再被 ok 覆盖日界——
            // 产品上「今日已尝试」即可，用 ok 锁日界。
            let _ = job::record_job_ok(&conn, "daily_digest");
            log::info!("daily_digest: nothing to say on {day}, marked ok");
            return Ok(());
        }
        let key = format!("daily_digest:{}", day.format("%Y-%m-%d"));
        crate::memory::db::set_setting(&conn, &key, &markdown).map_err(|e| e.to_string())?;
        let _ = job::record_job_ok(&conn, "daily_digest");
    }

    let preview: String = markdown.chars().take(80).collect();
    let _ = app.emit(
        "report:daily_ready",
        serde_json::json!({
            "date": day.format("%Y-%m-%d").to_string(),
            "preview": preview,
        }),
    );
    log::info!("daily_digest: ready for {day}");
    Ok(())
}

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

/// 兼容旧测试名：纯 consolidate 日界判定。
pub fn decide_trigger(
    hour: u32,
    today: chrono::NaiveDate,
    last_run_date: Option<chrono::NaiveDate>,
    hours_since_last: Option<f64>,
) -> Option<job::DueKind> {
    job::consolidate_due(hour, today, last_run_date, hours_since_last)
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
        assert_eq!(
            decide_trigger(3, d("2026-11-18"), Some(d("2026-11-17")), Some(15.0)),
            Some(DueKind::Nightly)
        );
        assert_eq!(
            decide_trigger(5, d("2026-11-18"), Some(d("2026-11-17")), Some(26.0)),
            Some(DueKind::Nightly)
        );
        assert_eq!(
            decide_trigger(2, d("2026-11-18"), Some(d("2026-11-17")), Some(20.0)),
            None
        );
    }

    #[test]
    fn never_twice_per_day() {
        assert_eq!(
            decide_trigger(4, d("2026-11-18"), Some(d("2026-11-18")), Some(1.0)),
            None
        );
        assert_eq!(
            decide_trigger(23, d("2026-11-18"), Some(d("2026-11-18")), Some(20.0)),
            None
        );
    }

    #[test]
    fn first_ever_run_fires_nightly() {
        assert_eq!(
            decide_trigger(10, d("2026-11-18"), None, None),
            Some(DueKind::Nightly)
        );
    }

    #[test]
    fn stale_last_run_catchups_even_before_run_hour() {
        assert_eq!(
            decide_trigger(2, d("2026-11-18"), Some(d("2026-11-16")), Some(40.0)),
            Some(DueKind::Catchup)
        );
    }

    #[test]
    fn yesterday_run_aged_36h_plus_catchups() {
        assert_eq!(
            decide_trigger(1, d("2026-11-18"), Some(d("2026-11-17")), Some(37.0)),
            Some(DueKind::Catchup)
        );
    }

    #[test]
    fn a8_consolidate_failure_leaves_a_row_and_a_backoff() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::memory::db::SCHEMA).unwrap();
        let guard = crate::failure::JobGuard { name: "consolidate" };
        let now = chrono::Local::now();

        let st = record_consolidate_failure(
            &conn,
            &guard,
            now,
            "nightly",
            "1970-01-01T00:00:00Z",
            "2026-11-24T00:00:00Z",
            "consolidate run failed: model down",
        );
        assert_eq!(st.fails, 1);

        let (status, err): (String, String) = conn
            .query_row(
                "SELECT status, error FROM consolidate_runs WHERE status='error' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "error");
        assert!(err.contains("model down"), "{err}");

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
            record_consolidate_failure(&conn, &guard, now, "nightly", "1970", "2026", "boom");
        }
        assert!(
            !guard.should_run(&conn, now + chrono::Duration::hours(4)),
            "当天剩余时间都要被挡"
        );
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM consolidate_runs WHERE status='error'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 5, "每次失败各留一行");
    }
}
