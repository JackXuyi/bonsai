//! Calendar cache, tool_calls audit, tasks / plan_json (v12–v14).

use rusqlite::{params, Connection, OptionalExtension};

use crate::memory::clock;

// ------------------------------------------------- calendar cache + tools audit
// (plan-calendar.md §3.2 — a disposable read cache and a content-free audit)

/// One cached calendar occurrence.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarRow {
    pub external_id: String,
    pub start_at: String,
    pub end_at: String,
    pub title: String,
    pub calendar_name: Option<String>,
    pub all_day: bool,
    pub attendee_count: i64,
    pub declined: bool,
    pub canceled: bool,
    pub availability: Option<String>,
    pub reminded_at: Option<String>,
    pub fetched_at: String,
}

const CAL_COLS: &str = "external_id, start_at, end_at, title, calendar_name, all_day, \
                        attendee_count, declined, canceled, availability, reminded_at, fetched_at";

fn calendar_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CalendarRow> {
    Ok(CalendarRow {
        external_id: row.get(0)?,
        start_at: row.get(1)?,
        end_at: row.get(2)?,
        title: row.get(3)?,
        calendar_name: row.get(4)?,
        all_day: row.get::<_, i64>(5)? != 0,
        attendee_count: row.get(6)?,
        declined: row.get::<_, i64>(7)? != 0,
        canceled: row.get::<_, i64>(8)? != 0,
        availability: row.get(9)?,
        reminded_at: row.get(10)?,
        fetched_at: row.get(11)?,
    })
}

/// Replace the cache with one fetch's worth of occurrences.
///
/// Upsert on `(external_id, start_at)` rather than delete-all: `reminded_at`
/// must survive a refresh, or the same meeting gets announced every 5 minutes.
/// Rows not present in this fetch are dropped by the `fetched_at` sweep — a
/// cancellation upstream must disappear here too.
pub fn calendar_replace(
    conn: &mut Connection,
    rows: &[CalendarRow],
    fetched_at: &str,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for r in rows {
        tx.execute(
            "INSERT INTO calendar_cache
               (external_id, start_at, end_at, title, calendar_name, all_day,
                attendee_count, declined, canceled, availability, fetched_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(external_id, start_at) DO UPDATE SET
                end_at = excluded.end_at,
                title = excluded.title,
                calendar_name = excluded.calendar_name,
                all_day = excluded.all_day,
                attendee_count = excluded.attendee_count,
                declined = excluded.declined,
                canceled = excluded.canceled,
                availability = excluded.availability,
                fetched_at = excluded.fetched_at",
            params![
                r.external_id,
                r.start_at,
                r.end_at,
                r.title,
                r.calendar_name,
                r.all_day as i64,
                r.attendee_count,
                r.declined as i64,
                r.canceled as i64,
                r.availability,
                fetched_at
            ],
        )?;
    }
    tx.execute("DELETE FROM calendar_cache WHERE fetched_at < ?1", params![fetched_at])?;
    tx.commit()
}

/// Cached occurrences overlapping `[start, end)`, oldest start first.
pub fn calendar_between(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<Vec<CalendarRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CAL_COLS} FROM calendar_cache
         WHERE end_at > ?1 AND start_at < ?2 ORDER BY start_at ASC"
    ))?;
    let rows = stmt.query_map(params![start_iso, end_iso], calendar_row)?;
    rows.collect()
}

/// The next occurrence starting at or after `after_iso`.
pub fn calendar_next_after(
    conn: &Connection,
    after_iso: &str,
) -> rusqlite::Result<Option<CalendarRow>> {
    conn.query_row(
        &format!(
            "SELECT {CAL_COLS} FROM calendar_cache
             WHERE start_at >= ?1 ORDER BY start_at ASC LIMIT 1"
        ),
        params![after_iso],
        calendar_row,
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// When the cache was last refreshed (`None` = never).
pub fn calendar_last_fetch(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT MAX(fetched_at) FROM calendar_cache", [], |r| {
        r.get::<_, Option<String>>(0)
    })
}

/// Mark one occurrence as reminded. Returns false when it already was —
/// the idempotency leg of the reminder lane.
pub fn mark_calendar_reminded(
    conn: &Connection,
    external_id: &str,
    start_at: &str,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE calendar_cache SET reminded_at = ?1
         WHERE external_id = ?2 AND start_at = ?3 AND reminded_at IS NULL",
        params![clock::now_iso(), external_id, start_at],
    )?;
    Ok(n > 0)
}

/// Drop the whole cache (switch off, permission loss, user request).
pub fn clear_calendar_cache(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM calendar_cache", [])
}

/// Append one audit row. `detail` carries counts/latency only.
pub fn log_tool_call(
    conn: &Connection,
    tool: &str,
    action: &str,
    outcome: &str,
    detail: Option<&str>,
    duration_ms: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tool_calls (ts, tool, action, outcome, detail, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![clock::now_iso(), tool, action, outcome, detail, duration_ms],
    )?;
    Ok(())
}

/// Agent-pipeline audit row (spec-s4): masked params summary + fine-grained
/// result status + a prompt-trace reference + remote flag. Content secrets
/// must already be masked by the caller — this fn never un-masks.
#[allow(clippy::too_many_arguments)]
pub fn log_agent_tool_call(
    conn: &Connection,
    tool: &str,
    action: &str,
    outcome: &str,
    result_status: &str,
    params_summary: Option<&str>,
    prompt_trace: Option<&str>,
    remote: bool,
    duration_ms: Option<i64>,
    task_id: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tool_calls
           (ts, tool, action, outcome, result_status, params_summary, prompt_trace, remote, duration_ms, task_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            clock::now_iso(),
            tool,
            action,
            outcome,
            result_status,
            params_summary,
            prompt_trace,
            remote as i64,
            duration_ms,
            task_id
        ],
    )?;
    Ok(())
}

/// Phase 3 A2：给刚写入的审计行打幂等键（调用方在 insert 后用 last_insert_rowid）。
pub fn set_tool_call_idempotency_key(
    conn: &Connection,
    tool_call_id: i64,
    key: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tool_calls SET idempotency_key = ?2 WHERE id = ?1",
        params![tool_call_id, key],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- tasks（任务卡外壳，v12）/ 计划（v14）

/// 计划步骤（docs/zh/plan-planning.md §4.2）。字段保持 snake_case——
/// 这是落库/协议契约，不是 UI 类型；前端类型在 T6 单独声明。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanStep {
    pub seq: i32,
    pub intent: String,
    #[serde(default)]
    pub tool_hint: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub attempts: i32,
    #[serde(default)]
    pub tool_call_id: Option<i64>,
    /// Phase 3 A2：跨进程重入幂等键（可选）。
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Phase 3 A2：`read` | `write` | `ask` | `unknown`（可选）。
    #[serde(default)]
    pub effect_class: Option<String>,
}

/// 结构化计划：写入 tasks.plan_json，供 trace 与 D6 进度面板消费。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanJson {
    #[serde(default = "plan_version_default")]
    pub version: i32,
    pub goal: String,
    #[serde(default)]
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub replans: i32,
}

fn plan_version_default() -> i32 {
    1
}

/// 一次云端工具循环开始：落 running 行，返回 id（tool_calls.task_id 关联用）。
pub fn task_start(conn: &Connection, goal: &str) -> rusqlite::Result<String> {
    task_start_in(conn, goal, None)
}

/// 带会话归属的建卡（v15）：`forget_session` 按 session_id 连带清任务行。
/// 单步任务也落 session_id——它和计划任务一样带着用户原话（goal）。
pub fn task_start_in(
    conn: &Connection,
    goal: &str,
    session_id: Option<&str>,
) -> rusqlite::Result<String> {
    let id = crate::mcp_admin::new_task_id();
    let clipped: String = goal.chars().take(80).collect();
    conn.execute(
        "INSERT INTO tasks (id, created_at, goal, status, session_id)
         VALUES (?1, ?2, ?3, 'running', ?4)",
        params![id, clock::now_iso(), clipped, session_id],
    )?;
    Ok(id)
}

/// 循环结束：写终态。`tool_calls_count` 为本次循环执行的调用数（模板兜底
/// 路径也如实记）。
pub fn task_finish(
    conn: &Connection,
    id: &str,
    status: &str,
    tool_calls_count: i64,
    final_reply: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET status = ?2, tool_calls = ?3, final_reply = ?4,
                finished_at = COALESCE(finished_at, ?5) WHERE id = ?1",
        params![id, status, tool_calls_count, final_reply, clock::now_iso()],
    )?;
    Ok(())
}

/// P4-4：任务是否「全部校验通过」——`done` 且（无计划或每步 status=done）。
pub fn task_fully_validated(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT status, COALESCE(plan_mode, 'none') FROM tasks WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((status, mode)) = row else {
        return Ok(false);
    };
    if status != "done" {
        return Ok(false);
    }
    if mode != "planned" {
        return Ok(true);
    }
    let Some(plan) = plan_get(conn, id)? else {
        return Ok(false);
    };
    if plan.steps.is_empty() {
        return Ok(false);
    }
    Ok(plan.steps.iter().all(|s| s.status == "done"))
}

/// 写计划 JSON（T2 计划轮产出 / T4 replan 覆盖）。
/// `plan_mode='planned'` 与 JSON 同事务写入，避免出现「有计划但模式是 none」。
pub fn plan_put(conn: &Connection, task_id: &str, plan: &PlanJson) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(plan)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "UPDATE tasks SET plan_json = ?2, plan_mode = 'planned' WHERE id = ?1",
        params![task_id, raw],
    )?;
    Ok(())
}

/// 读计划 JSON。解析失败视为「无计划」（降级为单步任务），不 panic——
/// 一条脏数据不能打挂 trace/进度面板（沿用 v10 教训的防御姿态）。
pub fn plan_get(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<PlanJson>> {
    let raw: Option<String> = conn.query_row(
        "SELECT plan_json FROM tasks WHERE id = ?1",
        params![task_id],
        |r| r.get(0),
    ).optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

/// 记录本轮确认模式（D1：`auto` 默认直放；`manual` 本版只留痕不阻塞）。
pub fn task_set_confirm_mode(conn: &Connection, task_id: &str, mode: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET confirm_mode = ?2 WHERE id = ?1",
        params![task_id, mode],
    )?;
    Ok(())
}

/// 记录取消原因（`user` / `budget`）。
pub fn task_set_cancel_reason(
    conn: &Connection,
    task_id: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET cancel_reason = ?2 WHERE id = ?1",
        params![task_id, reason],
    )?;
    Ok(())
}

/// 任务卡读取（UI 只读总览，沿 tool_call_records 惯例：最新在前）。
/// `plan` 是解析后的结构化计划（T6 `plan_list` 直接转发给前端）；
/// 脏 JSON 视为 None，不阻断列表。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub created_at: String,
    pub goal: String,
    pub status: String,
    pub tool_calls: i64,
    pub final_reply: Option<String>,
    pub plan: Option<PlanJson>,
    pub plan_mode: String,
    pub cancel_reason: Option<String>,
    pub confirm_mode: String,
    /// Phase 3 A1：可恢复标记（面板「恢复」按钮）。
    pub resume_cursor: Option<i32>,
    pub interrupt_kind: Option<String>,
    pub resumable: bool,
}

pub fn task_list(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<TaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, goal, status, tool_calls, final_reply,
                plan_json, plan_mode, cancel_reason, confirm_mode,
                resume_cursor, interrupt_kind
         FROM tasks ORDER BY rowid DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        let raw_plan: Option<String> = r.get(6)?;
        let status: String = r.get(3)?;
        let cancel_reason: Option<String> = r.get(8)?;
        let plan_mode: String = r.get::<_, Option<String>>(7)?.unwrap_or_else(|| "none".into());
        let resumable = plan_mode == "planned"
            && matches!(status.as_str(), "interrupted" | "failed" | "running")
            && cancel_reason.as_deref() != Some("user");
        Ok(TaskRow {
            id: r.get(0)?,
            created_at: r.get(1)?,
            goal: r.get(2)?,
            status,
            tool_calls: r.get(4)?,
            final_reply: r.get(5)?,
            plan: raw_plan.and_then(|s| serde_json::from_str(&s).ok()),
            plan_mode,
            cancel_reason,
            confirm_mode: r.get::<_, Option<String>>(9)?.unwrap_or_else(|| "auto".into()),
            resume_cursor: r.get(10)?,
            interrupt_kind: r.get(11)?,
            resumable,
        })
    })?;
    rows.collect()
}

/// One audit row as the UI reads it (spec-s6 tool-call records card).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub id: i64,
    pub ts: String,
    pub tool: String,
    pub action: String,
    pub outcome: String,
    pub result_status: Option<String>,
    pub params_summary: Option<String>,
    pub prompt_trace: Option<String>,
    pub remote: bool,
    pub duration_ms: Option<i64>,
}

/// Recent audit rows, newest first.
pub fn tool_call_records(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<ToolCallRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, tool, action, outcome, result_status, params_summary,
                prompt_trace, COALESCE(remote, 0), duration_ms
         FROM tool_calls ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(ToolCallRecord {
            id: r.get(0)?,
            ts: r.get(1)?,
            tool: r.get(2)?,
            action: r.get(3)?,
            outcome: r.get(4)?,
            result_status: r.get(5)?,
            params_summary: r.get(6)?,
            prompt_trace: r.get(7)?,
            remote: r.get::<_, i64>(8)? != 0,
            duration_ms: r.get(9)?,
        })
    })?;
    rows.collect()
}

/// `(ok_count, total_count)` for one tool on a local day window.
pub fn tool_calls_summary(
    conn: &Connection,
    tool: &str,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<(i64, i64)> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END), 0),
            COUNT(*)
         FROM tool_calls WHERE tool = ?1 AND ts >= ?2 AND ts < ?3",
        params![tool, start_iso, end_iso],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// Most recent audit row timestamp for one (tool, action) — the reminder
/// lane's "at least 10 minutes apart" gate reads this, not the cache.
pub fn last_tool_call_ts(
    conn: &Connection,
    tool: &str,
    action: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT MAX(ts) FROM tool_calls WHERE tool = ?1 AND action = ?2 AND outcome = 'ok'",
        params![tool, action],
        |r| r.get::<_, Option<String>>(0),
    )
}

/// Most recent audit rows for the tools page: `(ts, tool, action, outcome, detail)`.
pub fn tool_calls_recent(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, String, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT ts, tool, action, outcome, detail FROM tool_calls
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })?;
    rows.collect()
}

