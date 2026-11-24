//! P4-4 — 能力线（docs/zh/plan-ability-line/v1）。
//!
//! 形态线改外形，关系线改怎么说话；能力线改「能不能办事」——证据只来自
//! 任务卡结算质量（全部校验通过 / 失败 / 取消），不靠签到。
//!
//! 不变量（对齐关系线哲学）：
//! - 阶段只升不降。
//! - 日证据从 `tasks.finished_at` **重算**，不在 finish 路径累加计数器。
//! - 分数永不离开本进程：无 API 字段、无 UI、无日志外露。
//! - 取消不计分（对齐 shut_up：用户行使控制不该被惩罚）。
//! - 不新开第 9 条 loop：挂在 relationship tick 后同线程共跑（LOOP_NAMES=8）。

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::db;

/// (min_day, min_cum)。双条件；cum 不含今日。
pub const STAGE_THRESHOLDS: [(i64, i64); 3] = [(0, 0), (31, 15), (60, 40)];

pub const STAGE_NAMES: [&str; 3] = ["陪聊", "能办事", "真帮手"];

pub const EVIDENCE_WINDOW_DAYS: i64 = 60;

// ---------------------------------------------------------------- derivation

/// 日分：present +1、validated +1、failed −1、canceled 0。结果 ∈ [-1, 2]。
pub fn score_day(present: bool, validated: i64, failed: i64, _canceled: i64) -> i64 {
    let mut s = 0i64;
    if present {
        s += 1;
    }
    if validated > 0 {
        s += 1;
    }
    if failed > 0 {
        s -= 1;
    }
    s
}

pub fn derive_stage(day_index: i64, cum: i64) -> i64 {
    let mut stage = 0;
    for (i, (min_day, min_cum)) in STAGE_THRESHOLDS.iter().enumerate() {
        if day_index >= *min_day && cum >= *min_cum {
            stage = i as i64;
        }
    }
    stage
}

fn announcement(from: i64, to: i64, day_index: i64) -> String {
    if from == 0 && to >= 2 {
        return "其实你让我办成过不少事了。行，往后我按真帮手来。".into();
    }
    let variant = (day_index.max(0) as usize) % 2;
    match (to, variant) {
        (1, 0) => "办成几件事了。我开始摸清怎么帮你落地。".into(),
        (1, _) => "能办事了——你给目标，我按步骤走完。".into(),
        (2, 0) => "真帮手了。计划、校验、收尾，我按这个节奏来。".into(),
        _ => "帮你办成的事够多了。以后我更敢主动把步骤走完。".into(),
    }
}

// ---------------------------------------------------------------- evidence

/// 从 tasks 重算本地日证据（finished_at 落在该日）。
pub fn day_evidence(
    conn: &Connection,
    day: NaiveDate,
    now_local: DateTime<chrono::Local>,
) -> rusqlite::Result<db::AbilityDayEvidence> {
    let start = local_midnight(day, now_local.offset()).to_rfc3339();
    let end = local_midnight(day + Duration::days(1), now_local.offset()).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, status FROM tasks
         WHERE finished_at IS NOT NULL
           AND finished_at >= ?1 AND finished_at < ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![start, end], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;

    let mut present = false;
    let mut validated = 0i64;
    let mut failed = 0i64;
    let mut canceled = 0i64;
    for row in rows {
        let (id, status) = row?;
        present = true;
        match status.as_str() {
            "failed" => failed += 1,
            "canceled" => canceled += 1,
            _ => {}
        }
        if db::task_fully_validated(conn, &id)? {
            validated += 1;
        }
    }

    Ok(db::AbilityDayEvidence {
        present: i64::from(present),
        validated,
        failed,
        canceled,
        score: score_day(present, validated, failed, canceled),
    })
}

fn local_midnight(day: NaiveDate, offset: &chrono::FixedOffset) -> DateTime<Utc> {
    let naive = day.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    offset
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

/// 回放最近 `days` 天；幂等 UPSERT。
pub fn backfill(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
    days: i64,
) -> rusqlite::Result<usize> {
    let today = now_local.date_naive();
    let mut n = 0;
    for back in 0..days {
        let day = today - Duration::days(back);
        let ev = day_evidence(conn, day, now_local)?;
        db::upsert_ability_progress(conn, &day.to_string(), &ev)?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------- tick

#[derive(Debug, Default, PartialEq)]
pub struct TickOutcome {
    pub unlocked: Option<(i64, String)>,
    pub stage: i64,
    pub day_index: i64,
}

pub fn tick(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
) -> rusqlite::Result<TickOutcome> {
    let state = db::ensure_ability(conn)?;
    let today = now_local.format("%Y-%m-%d").to_string();
    let today_naive = now_local.date_naive();
    let mut out = TickOutcome {
        stage: state.stage,
        ..Default::default()
    };

    let ev = day_evidence(conn, today_naive, now_local)?;
    db::upsert_ability_progress(conn, &today, &ev)?;

    let growth = db::ensure_growth(conn)?;
    let di = crate::morph::day_index(&growth.born_at, now_local);
    out.day_index = di;

    if state.last_tick_date.as_deref() != Some(today.as_str()) {
        let from = today_naive - Duration::days(EVIDENCE_WINDOW_DAYS);
        let cum = db::ability_progress_sum_between(conn, &from.to_string(), &today)?;
        let target = derive_stage(di, cum);
        if target > state.stage {
            let note = announcement(state.stage, target, di);
            db::set_ability_stage(conn, target)?;
            db::add_ability_log(conn, target, &note)?;
            db::add_event(
                conn,
                "ability_stage",
                &format!("{{\"stage\":{target},\"day\":{di}}}"),
            )?;
            out.unlocked = Some((target, note));
        }
        db::set_ability_tick_date(conn, &today)?;
        out.stage = db::ability_state(conn)?
            .map(|s| s.stage)
            .unwrap_or(state.stage);
    }
    Ok(out)
}

/// relationship 同线程共跑：boot 回填 + 每次 tick。返回是否解锁（供 emit）。
pub fn cotick_with_relationship(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
    boot_backfill: bool,
) -> Option<AbilityStatus> {
    if boot_backfill {
        let _ = backfill(conn, now_local, EVIDENCE_WINDOW_DAYS);
    }
    match tick(conn, now_local) {
        Ok(out) if out.unlocked.is_some() => Some(status_payload(conn)),
        _ => None,
    }
}

// ---------------------------------------------------------------- status

#[derive(Debug, Clone, Serialize)]
pub struct AbilityStatus {
    pub stage: i64,
    pub stage_name: String,
    pub day_index: i64,
    pub stage_since: Option<String>,
    pub pending: Vec<PendingAbility>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingAbility {
    pub id: i64,
    pub stage: i64,
    pub note: String,
}

pub fn status_payload(conn: &Connection) -> AbilityStatus {
    let state = db::ensure_ability(conn).ok();
    let growth = db::ensure_growth(conn).ok();
    let now = chrono::Local::now();
    let day_index = growth
        .as_ref()
        .map(|g| crate::morph::day_index(&g.born_at, now))
        .unwrap_or(0);
    let stage = state.as_ref().map(|s| s.stage).unwrap_or(0);
    let pending = db::pending_ability_logs(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, stage, note)| PendingAbility { id, stage, note })
        .collect();
    AbilityStatus {
        stage,
        stage_name: STAGE_NAMES
            .get(stage as usize)
            .unwrap_or(&"陪聊")
            .to_string(),
        day_index,
        stage_since: state.and_then(|s| s.stage_since),
        pending,
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn ability_status(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<AbilityStatus, String> {
    store
        .inner()
        .with_read(|conn| Ok(status_payload(conn)))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ability_ack(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: i64,
) -> Result<bool, String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    db::mark_ability_notified(&conn, id).map_err(|e| e.to_string())
}

/// 只清能力线；记忆 / 盆栽 / 关系不动。
#[tauri::command]
pub fn ability_reset(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<(), String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    db::forget_ability(&conn).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;
    use crate::memory::db::{PlanJson, PlanStep};

    fn mem() -> Connection {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join("bonsai-ability-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test-{}-{n}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        db::open(&path).unwrap()
    }

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .unwrap()
    }

    fn born_on(conn: &Connection, iso: &str) {
        db::ensure_growth(conn).unwrap();
        conn.execute(
            "UPDATE growth SET born_at = ?1 WHERE id = 1",
            rusqlite::params![iso],
        )
        .unwrap();
    }

    fn finish_task(
        conn: &Connection,
        goal: &str,
        status: &str,
        finished_at: &str,
        plan: Option<PlanJson>,
    ) -> String {
        let id = db::task_start(conn, goal).unwrap();
        if let Some(p) = plan {
            db::plan_put(conn, &id, &p).unwrap();
        }
        db::task_finish(conn, &id, status, 1, Some("ok")).unwrap();
        conn.execute(
            "UPDATE tasks SET finished_at = ?1 WHERE id = ?2",
            rusqlite::params![finished_at, id],
        )
        .unwrap();
        id
    }

    #[test]
    fn stage_needs_day_and_cum_together() {
        assert_eq!(derive_stage(0, 0), 0);
        assert_eq!(derive_stage(30, 99), 0, "day 30 too early");
        assert_eq!(derive_stage(31, 14), 0, "one short of cum");
        assert_eq!(derive_stage(31, 15), 1);
        assert_eq!(derive_stage(59, 99), 1);
        assert_eq!(derive_stage(60, 39), 1);
        assert_eq!(derive_stage(60, 40), 2);
        assert_eq!(derive_stage(365, 999), 2);
    }

    #[test]
    fn scoring_never_punishes_cancel() {
        assert_eq!(score_day(true, 0, 0, 3), 1);
        assert_eq!(score_day(true, 1, 0, 0), 2);
        assert_eq!(score_day(true, 0, 2, 0), 0);
        assert_eq!(score_day(false, 0, 0, 0), 0);
        assert_eq!(score_day(true, 1, 1, 0), 1);
    }

    /// A1：计划全部 step=done 且 status=done → validated≥1
    #[test]
    fn validated_when_all_plan_steps_done() {
        let _c = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let now = local(2026, 11, 19, 10);
        let plan = PlanJson {
            version: 1,
            goal: "合并摘要".into(),
            steps: vec![
                PlanStep {
                    seq: 1,
                    intent: "读".into(),
                    tool_hint: String::new(),
                    status: "done".into(),
                    attempts: 1,
                    tool_call_id: None,
                    idempotency_key: None,
                    effect_class: None,
                },
                PlanStep {
                    seq: 2,
                    intent: "写".into(),
                    tool_hint: String::new(),
                    status: "done".into(),
                    attempts: 1,
                    tool_call_id: None,
                    idempotency_key: None,
                    effect_class: None,
                },
            ],
            replans: 0,
        };
        // finished_at 用本地日窗口内的 UTC 戳
        let fin = local_midnight(now.date_naive(), now.offset()) + Duration::hours(3);
        finish_task(
            &conn,
            "合并摘要",
            "done",
            &fin.to_rfc3339(),
            Some(plan),
        );
        let ev = day_evidence(&conn, now.date_naive(), now).unwrap();
        assert_eq!(ev.present, 1);
        assert!(ev.validated >= 1, "A1 validated={}", ev.validated);
        assert_eq!(ev.failed, 0);
        assert_eq!(ev.score, 2);
    }

    /// A2：status=failed → 当日 score 可 −1（相对仅 present）
    #[test]
    fn failed_task_costs_a_point() {
        let _c = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let now = local(2026, 11, 19, 10);
        let fin = local_midnight(now.date_naive(), now.offset()) + Duration::hours(4);
        finish_task(&conn, "坏了", "failed", &fin.to_rfc3339(), None);
        let ev = day_evidence(&conn, now.date_naive(), now).unwrap();
        assert_eq!(ev.present, 1);
        assert_eq!(ev.failed, 1);
        assert_eq!(ev.score, 0, "present+1 failed-1 → 0");
    }

    /// A3：同日源数据不变 → UPSERT 幂等
    #[test]
    fn progress_upsert_is_idempotent() {
        let _c = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let now = local(2026, 11, 19, 10);
        let fin = local_midnight(now.date_naive(), now.offset()) + Duration::hours(2);
        finish_task(&conn, "ok", "done", &fin.to_rfc3339(), None);
        let a = tick(&conn, now).unwrap();
        let b = tick(&conn, local(2026, 11, 19, 15)).unwrap();
        assert_eq!(a.unlocked, None);
        assert_eq!(b.unlocked, None);
        let day = now.format("%Y-%m-%d").to_string();
        let score: i64 = conn
            .query_row(
                "SELECT score FROM ability_progress WHERE date = ?1",
                rusqlite::params![day],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(score, 2, "done 无计划 = present+validated");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM ability_progress", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    /// A4：双门槛未齐 → 不升阶
    #[test]
    fn dual_gate_blocks_promotion() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        // 累计够但天数不够
        for d in 1..=10 {
            db::upsert_ability_progress(
                &conn,
                &format!("2026-11-{d:02}"),
                &db::AbilityDayEvidence {
                    present: 1,
                    validated: 1,
                    score: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let _c2 = clock::pinned("2026-11-20T02:00:00+00:00");
        let out = tick(&conn, local(2026, 11, 20, 10)).unwrap();
        assert_eq!(out.stage, 0, "day_index≈19 < 31");
        assert!(out.unlocked.is_none());

        // 天数够但 cum 不够
        let conn2 = mem();
        born_on(&conn2, "2026-09-01T02:00:00+00:00");
        db::upsert_ability_progress(
            &conn2,
            "2026-11-01",
            &db::AbilityDayEvidence {
                present: 1,
                validated: 1,
                score: 10,
                ..Default::default()
            },
        )
        .unwrap();
        let _c3 = clock::pinned("2026-11-20T02:00:00+00:00");
        let out2 = tick(&conn2, local(2026, 11, 20, 10)).unwrap();
        assert_eq!(out2.stage, 0, "cum 10 < 15");
    }

    #[test]
    fn promotion_once_per_day_when_gates_met() {
        let _c = clock::pinned("2026-09-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-09-01T02:00:00+00:00");
        for d in 1..=15 {
            db::upsert_ability_progress(
                &conn,
                &format!("2026-10-{d:02}"),
                &db::AbilityDayEvidence {
                    present: 1,
                    validated: 1,
                    score: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        // day_index from Sep 1 to Nov 2 ≈ 62
        let _c2 = clock::pinned("2026-11-02T02:00:00+00:00");
        let out = tick(&conn, local(2026, 11, 2, 10)).unwrap();
        assert!(out.stage >= 1, "day≥31 + cum≥15 → ≥能办事, got {}", out.stage);
        assert!(out.unlocked.is_some());
        let out2 = tick(&conn, local(2026, 11, 2, 15)).unwrap();
        assert!(out2.unlocked.is_none());
        assert_eq!(db::pending_ability_logs(&conn).unwrap().len(), 1);
    }
}
