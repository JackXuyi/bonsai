//! Dialogue history reads + turn_trace / trace_steps (spec-s8 / plan-context).

use rusqlite::{params, Connection, OptionalExtension};

use crate::memory::clock;

/// 一轮对话 + 摘要需要的元数据（plan-context §6.4）。
///
/// `upto_id` 是该轮的**收尾事件 id**（assistant 行）：摘要幂等指针
/// `sessions.digest_upto_event_id` 按它前进，重复装配不会重复合并同一轮。
/// `day` 是本地「MM-DD」，只在摘要行前缀里用。
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub user: String,
    pub assistant: String,
    pub upto_id: i64,
    pub day: String,
}

/// 抽取 `max_turns` 轮对话的配对逻辑（唯一实现）。
///
/// 配对规则：连续的 user → assistant 拼一轮；没有前导 user 的 assistant 行
/// 是主动搭话（用合成 user 标号保留，让后面对话仍知道「它自己说过什么」）；
/// 没有 assistant 回复的 user 行丢掉（模型那一轮死在半路，或者已被窗口截断）。
pub fn dialogue_turns_meta(
    conn: &Connection,
    session_id: &str,
    max_turns: usize,
) -> rusqlite::Result<Vec<TurnMeta>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, role, content FROM events
         WHERE session_id = ?1 AND kind = 'dialogue'
           AND role IN ('user', 'assistant')
         ORDER BY id DESC LIMIT ?2",
    )?;
    let mut rows: Vec<(i64, String, String, String)> = stmt
        .query_map(params![session_id, (max_turns * 2) as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    rows.reverse(); // oldest first

    let mut out = Vec::new();
    let mut pending_user: Option<String> = None;
    for (id, ts, role, content) in rows {
        match role.as_str() {
            "user" => pending_user = Some(content),
            "assistant" => {
                let user =
                    pending_user.take().unwrap_or_else(|| "(盆栽主动说的)".to_string());
                out.push(TurnMeta {
                    user,
                    assistant: content,
                    upto_id: id,
                    day: crate::memory::clock::day_mmdd(&ts),
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Last `max_turns` dialogue turns for a session, oldest first. Dialogue
/// events carry kind="dialogue", role user/assistant, and a session id.
pub fn dialogue_history(
    conn: &Connection,
    session_id: &str,
    max_turns: usize,
) -> rusqlite::Result<Vec<crate::persona::eval::Turn>> {
    Ok(
        dialogue_turns_meta(conn, session_id, max_turns)?
            .into_iter()
            .map(|t| crate::persona::eval::Turn {
                user: t.user,
                assistant: t.assistant,
            })
            .collect(),
    )
}

/// One dialogue row, flat, for the chat page's startup replay (spec: 对话页
/// 回放最近消息流). Deliberately NOT the paired `Turn` shape — the UI wants
/// every user/assistant row as its own bubble, including proactive ones.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DialogueBubble {
    pub id: i64,
    pub ts: String,
    pub role: String, // 'user' | 'assistant'
    pub content: String,
}

/// Last `limit` dialogue rows for a session, oldest first. Proactive
/// assistant rows are included (they live in the same events stream).
/// `before_id` is the pagination cursor for the chat page's scroll-up
/// loading: only rows with `id < before_id` are returned, so successive
/// pages tile the stream without overlap.
pub fn dialogue_bubbles(
    conn: &Connection,
    session_id: &str,
    limit: usize,
    before_id: Option<i64>,
) -> rusqlite::Result<Vec<DialogueBubble>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, role, content FROM events
         WHERE session_id = ?1 AND kind = 'dialogue'
           AND role IN ('user', 'assistant')
           AND (?3 IS NULL OR id < ?3)
         ORDER BY id DESC LIMIT ?2",
    )?;
    let mut rows: Vec<DialogueBubble> = stmt
        .query_map(params![session_id, limit as i64, before_id], |row| {
            Ok(DialogueBubble {
                id: row.get(0)?,
                ts: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    rows.reverse(); // oldest first
    Ok(rows)
}

// ---------------------------------------------------------------- turn trace (spec-s8)

/// One pipeline step, as the dialogue layer collects it in memory.
#[derive(Debug, Clone)]
pub struct TraceStepDraft {
    pub step_kind: String,
    pub title: String,
    pub prompt: Option<String>,
    pub output: Option<String>,
    pub args_json: Option<String>,
    pub summary: Option<String>,
    pub outcome: Option<String>,
    pub duration_ms: Option<i64>,
    pub user_text: Option<String>,
}

/// A whole turn's trace, persisted atomically (turn row + all steps).
#[derive(Debug, Clone)]
pub struct TraceTurnDraft {
    pub session_id: String,
    pub kind: String,
    pub model_used: bool,
    pub tool_label: Option<String>,
    pub filter_report: Option<String>,
    pub duration_ms: Option<i64>,
    pub user_text: Option<String>,
    pub reply_text: Option<String>,
    /// v17：本轮失败类别（正常轮 None）。来源：error llm_round 步的 output。
    pub error_kind: Option<String>,
    /// v17：重试次数（正常轮 None）。
    pub attempts: Option<i64>,
    pub steps: Vec<TraceStepDraft>,
}

/// One step as the UI reads it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceStepRow {
    pub seq: i64,
    pub step_kind: String,
    pub title: String,
    pub prompt: Option<String>,
    pub output: Option<String>,
    pub args_json: Option<String>,
    pub summary: Option<String>,
    pub outcome: Option<String>,
    pub duration_ms: Option<i64>,
    pub user_text: Option<String>,
    pub ts: String,
}

/// One turn (master row + ordered steps) as the UI reads it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTraceRow {
    pub id: i64,
    pub session_id: String,
    pub user_event_id: Option<i64>,
    pub assistant_event_id: Option<i64>,
    pub kind: String,
    pub model_used: bool,
    pub tool_label: Option<String>,
    pub filter_report: Option<String>,
    pub duration_ms: Option<i64>,
    pub user_text: Option<String>,
    pub reply_text: Option<String>,
    pub ts: String,
    pub steps: Vec<TraceStepRow>,
}

/// Persist a finished turn: master row + all steps in ONE transaction so a
/// crash mid-write can never leave a master row without its steps. Returns
/// the turn id. Must be called in the SAME lock scope that inserted the
/// user/assistant dialogue events (spec-s8 §3.2).
pub fn trace_turn_persist(
    conn: &Connection,
    turn: &TraceTurnDraft,
    user_event_id: Option<i64>,
    assistant_event_id: Option<i64>,
) -> rusqlite::Result<i64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO turn_trace
           (session_id, user_event_id, assistant_event_id, kind, model_used,
            tool_label, filter_report, duration_ms, user_text, reply_text,
            error_kind, attempts, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            turn.session_id,
            user_event_id,
            assistant_event_id,
            turn.kind,
            turn.model_used as i64,
            turn.tool_label,
            turn.filter_report,
            turn.duration_ms,
            turn.user_text,
            turn.reply_text,
            turn.error_kind,
            turn.attempts,
            clock::now_iso(),
        ],
    )?;
    let turn_id = tx.last_insert_rowid();
    for (i, step) in turn.steps.iter().enumerate() {
        tx.execute(
            "INSERT INTO trace_steps
               (turn_id, seq, step_kind, title, prompt, output, args_json, summary, outcome, duration_ms, user_text, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                turn_id,
                (i + 1) as i64,
                step.step_kind,
                step.title,
                step.prompt,
                step.output,
                step.args_json,
                step.summary,
                step.outcome,
                step.duration_ms,
                step.user_text,
                clock::now_iso(),
            ],
        )?;
    }
    tx.commit()?;
    Ok(turn_id)
}

/// Full trace of one turn (None = turn id unknown — pre-trace message).
pub fn turn_trace_get(conn: &Connection, turn_id: i64) -> rusqlite::Result<Option<TurnTraceRow>> {
    let turn = conn
        .query_row(
            "SELECT id, session_id, user_event_id, assistant_event_id, kind,
                    model_used, tool_label, filter_report, duration_ms,
                    user_text, reply_text, ts
             FROM turn_trace WHERE id = ?1",
            params![turn_id],
            |r| {
                Ok(TurnTraceRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    user_event_id: r.get(2)?,
                    assistant_event_id: r.get(3)?,
                    kind: r.get(4)?,
                    model_used: r.get::<_, i64>(5)? != 0,
                    tool_label: r.get(6)?,
                    filter_report: r.get(7)?,
                    duration_ms: r.get(8)?,
                    user_text: r.get(9)?,
                    reply_text: r.get(10)?,
                    ts: r.get(11)?,
                    steps: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut turn) = turn else { return Ok(None) };
    let mut stmt = conn.prepare(
        "SELECT seq, step_kind, title, prompt, output, args_json, summary,
                outcome, duration_ms, user_text, ts
         FROM trace_steps WHERE turn_id = ?1 ORDER BY seq ASC",
    )?;
    turn.steps = stmt
        .query_map(params![turn_id], |r| {
            Ok(TraceStepRow {
                seq: r.get(0)?,
                step_kind: r.get(1)?,
                title: r.get(2)?,
                prompt: r.get(3)?,
                output: r.get(4)?,
                args_json: r.get(5)?,
                summary: r.get(6)?,
                outcome: r.get(7)?,
                duration_ms: r.get(8)?,
                user_text: r.get(9)?,
                ts: r.get(10)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(Some(turn))
}

/// Reverse lookup: which turn produced this dialogue event (either side).
/// Returns None for pre-trace messages.
pub fn turn_trace_find_by_event(conn: &Connection, event_id: i64) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM turn_trace
         WHERE user_event_id = ?1 OR assistant_event_id = ?1
         ORDER BY id DESC LIMIT 1",
        params![event_id],
        |r| r.get(0),
    )
    .optional()
}
