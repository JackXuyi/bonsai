//! Session index + digest (plan-context §5 / §6).

use rusqlite::{params, Connection, OptionalExtension};

use crate::memory::clock;

// ------------------------------------------------------ sessions (v15)

/// 会话索引行（plan-context §5.1）。`title` 空串 = 未命名，前端显示「新对话」。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub archived: bool,
    pub message_count: i64,
    pub digest: Option<String>,
    pub digest_upto_event_id: Option<i64>,
}

/// 自动标题：首条用户消息前 24 字，换行折成空格。全空白 → None（不改名，
/// 留给下一条有内容的用户消息）。
const AUTO_TITLE_MAX_CHARS: usize = 24;
/// 手动改名上限（比自动命名宽松：这是用户自己的话）。
const MANUAL_TITLE_MAX_CHARS: usize = 40;

fn auto_title(text: &str) -> Option<String> {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.is_empty() {
        return None;
    }
    let clipped: String = trimmed.chars().take(AUTO_TITLE_MAX_CHARS).collect();
    Some(clipped)
}

/// 对话事件落地时维护会话索引：确保行存在、touch updated_at、累加条数，
/// 并在**还没被命名**（title = ''）时用首条用户消息命名。
/// 手动改过的标题永不被覆盖——判断依据就是「非空」，不用中文字符串哨兵。
pub fn session_touch(
    conn: &Connection,
    session_id: &str,
    user_text: Option<&str>,
) -> rusqlite::Result<()> {
    let now = clock::now_iso();
    let candidate = user_text.and_then(auto_title);
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, updated_at, message_count)
         VALUES (?1, ?2, ?3, ?3, 1)
         ON CONFLICT(id) DO UPDATE SET
             updated_at = excluded.updated_at,
             message_count = sessions.message_count + 1",
        params![session_id, candidate.clone().unwrap_or_default(), now],
    )?;
    if let Some(t) = candidate {
        conn.execute(
            "UPDATE sessions SET title = ?2 WHERE id = ?1 AND title = ''",
            params![session_id, t],
        )?;
    }
    Ok(())
}

const SESSION_COLUMNS: &str =
    "id, title, created_at, updated_at, archived, message_count, digest, digest_upto_event_id";

fn session_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        title: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        archived: r.get::<_, i64>(4)? != 0,
        message_count: r.get(5)?,
        digest: r.get(6)?,
        digest_upto_event_id: r.get(7)?,
    })
}

/// 会话列表：未归档在前，`updated_at` 倒序（同秒再用 rowid 兜底）。
/// `include_archived` 为 false 时只返回未归档（抽屉默认视图）。
pub fn session_list(conn: &Connection, include_archived: bool) -> rusqlite::Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions
          WHERE (?1 = 1 OR archived = 0)
          ORDER BY archived ASC, updated_at DESC, rowid DESC"
    ))?;
    let rows = stmt.query_map(params![include_archived as i64], session_from_row)?;
    rows.collect()
}

pub fn session_get(conn: &Connection, id: &str) -> rusqlite::Result<Option<SessionRow>> {
    conn.query_row(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1"),
        params![id],
        session_from_row,
    )
    .optional()
}

/// 新建会话。`id` 缺省由调用方生成；`title` 留给「新对话」以外的显式命名，
/// 空串 = 未命名（首条用户消息会补上）。
pub fn session_create(conn: &Connection, id: &str, title: &str) -> rusqlite::Result<SessionRow> {
    let now = clock::now_iso();
    let title: String = title.trim().chars().take(MANUAL_TITLE_MAX_CHARS).collect();
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, updated_at, message_count)
         VALUES (?1, ?2, ?3, ?3, 0)",
        params![id, title, now],
    )?;
    session_get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// 改名（用户显式操作，覆盖任何自动标题）。
pub fn session_rename(conn: &Connection, id: &str, title: &str) -> rusqlite::Result<usize> {
    let title: String = title.trim().chars().take(MANUAL_TITLE_MAX_CHARS).collect();
    conn.execute(
        "UPDATE sessions SET title = ?2 WHERE id = ?1",
        params![id, title],
    )
}

pub fn session_set_archived(
    conn: &Connection,
    id: &str,
    archived: bool,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE sessions SET archived = ?2 WHERE id = ?1",
        params![id, archived as i64],
    )
}

// ------------------------------------------------ digest（plan-context §6.4/§6.5）

/// 会话摘要的当前值：`(正文, 已合并到的最大 events.id)`。
/// 行不存在（proactive / 刚清库）→ `(None, None)`，调用方按「还没压过」处理。
pub fn session_digest(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<(Option<String>, Option<i64>)> {
    conn.query_row(
        "SELECT digest, digest_upto_event_id FROM sessions WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map(|o| o.unwrap_or((None, None)))
}

/// 写回摘要 + 幂等指针。**指针只前进不后退**（`CASE` 而不是直接赋值）：
/// 装配是并发路径，迟到的旧轮不该把水位线拖回去。返回受影响行数——
/// 0 说明会话行不存在（proactive），调用方忽略即可。
pub fn session_set_digest(
    conn: &Connection,
    id: &str,
    digest: &str,
    upto_id: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE sessions SET digest = ?2,
             digest_upto_event_id = CASE
                 WHEN COALESCE(digest_upto_event_id, 0) < ?3 THEN ?3
                 ELSE digest_upto_event_id END
         WHERE id = ?1",
        params![id, digest, upto_id],
    )
}

/// 比较并写回（L2 用，§6.5）：只有库里还是 `expect` 时才覆盖。
/// 夜间重写的几秒里，L1 可能又追加了新行——直接覆盖等于把新记忆吞了。
pub fn session_set_digest_if(
    conn: &Connection,
    id: &str,
    expect: &str,
    next: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE sessions SET digest = ?3 WHERE id = ?1 AND digest = ?2",
        params![id, expect, next],
    )
}

/// L2 的候选会话：有摘要、未归档、最近 `since` 之后有活动（§6.5「昨天有
/// digest 更新的会话」）。用 `updated_at` 近似：摘要更新本身不 touch，
/// 但真正需要重写的会话一定还在被使用。
pub fn session_digest_candidates(
    conn: &Connection,
    since: &str,
    limit: usize,
) -> rusqlite::Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions
          WHERE archived = 0 AND digest IS NOT NULL AND TRIM(digest) <> ''
            AND updated_at >= ?1
          ORDER BY updated_at DESC, rowid DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![since, limit as i64], session_from_row)?;
    rows.collect()
}

