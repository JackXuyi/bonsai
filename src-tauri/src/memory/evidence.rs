//! Memory provenance（docs/zh/plan-reliability-phase1/v1 R1）。
//!
//! 所有派生记忆（fact / preference / boundary / profile）通过本表挂到
//! 原始 `events`。Forget Service 只认这里的边，不靠各表零散 FK 猜来源。
//!
//! v19：`independence_group` —— 同组内对同一 fact 不重复 bump（plan-intelligence I3）。

use rusqlite::{params, Connection, OptionalExtension};

use super::clock;

pub const TYPE_FACT: &str = "fact";
pub const TYPE_PREFERENCE: &str = "preference";
pub const TYPE_BOUNDARY: &str = "boundary";
pub const TYPE_PROFILE: &str = "profile";

pub const REL_EXTRACTED: &str = "extracted";
pub const REL_PROMOTED: &str = "promoted";
pub const REL_DERIVED: &str = "derived";
pub const REL_MANUAL_LINK: &str = "manual_link";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRef {
    pub memory_type: String,
    pub memory_id: String,
}

/// 由 event 推导独立性组：优先 session_id，否则 `event:{id}`。
pub fn group_for_event(conn: &Connection, event_id: i64) -> rusqlite::Result<String> {
    let session = conn
        .query_row(
            "SELECT session_id FROM events WHERE id = ?1",
            params![event_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(match session {
        Some(s) if !s.is_empty() => s,
        _ => format!("event:{event_id}"),
    })
}

/// 幂等挂一条证据。`event_id` 必须已存在。
pub fn link(
    conn: &Connection,
    memory_type: &str,
    memory_id: &str,
    event_id: i64,
    relation: &str,
) -> rusqlite::Result<()> {
    let group = group_for_event(conn, event_id)?;
    link_with_group(conn, memory_type, memory_id, event_id, relation, &group)
}

pub fn link_with_group(
    conn: &Connection,
    memory_type: &str,
    memory_id: &str,
    event_id: i64,
    relation: &str,
    independence_group: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO memory_evidence
             (memory_type, memory_id, event_id, relation, created_at, independence_group)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            memory_type,
            memory_id,
            event_id,
            relation,
            clock::now_iso(),
            independence_group
        ],
    )?;
    Ok(())
}

pub fn link_fact(conn: &Connection, fact_id: i64, event_id: i64) -> rusqlite::Result<()> {
    link(
        conn,
        TYPE_FACT,
        &fact_id.to_string(),
        event_id,
        REL_EXTRACTED,
    )
}

pub fn link_preference(conn: &Connection, pref_id: i64, event_id: i64) -> rusqlite::Result<()> {
    link(
        conn,
        TYPE_PREFERENCE,
        &pref_id.to_string(),
        event_id,
        REL_PROMOTED,
    )
}

pub fn link_boundary(conn: &Connection, boundary_id: i64, event_id: i64) -> rusqlite::Result<()> {
    link(
        conn,
        TYPE_BOUNDARY,
        &boundary_id.to_string(),
        event_id,
        REL_DERIVED,
    )
}

pub fn link_profile(conn: &Connection, key: &str, event_id: i64, relation: &str) -> rusqlite::Result<()> {
    link(conn, TYPE_PROFILE, key, event_id, relation)
}

/// 该 fact 是否已有给定 independence_group 的 evidence。
pub fn fact_has_group(conn: &Connection, fact_id: i64, group: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_evidence
          WHERE memory_type = ?1 AND memory_id = ?2 AND independence_group = ?3",
        params![TYPE_FACT, fact_id.to_string(), group],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 一批 event 上挂着的全部派生记忆（去重）。
pub fn memories_for_events(
    conn: &Connection,
    event_ids: &[i64],
) -> rusqlite::Result<Vec<EvidenceRef>> {
    if event_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(event_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT memory_type, memory_id FROM memory_evidence
          WHERE event_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(event_ids.iter()), |row| {
        Ok(EvidenceRef {
            memory_type: row.get(0)?,
            memory_id: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// 删除指定 event 集上的 evidence 行，返回影响行数。
pub fn unlink_events(conn: &Connection, event_ids: &[i64]) -> rusqlite::Result<usize> {
    if event_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat("?")
        .take(event_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM memory_evidence WHERE event_id IN ({placeholders})");
    Ok(conn.execute(&sql, rusqlite::params_from_iter(event_ids.iter()))?)
}

/// 该派生记忆是否还有任意 evidence。
pub fn has_any(conn: &Connection, memory_type: &str, memory_id: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_evidence
          WHERE memory_type = ?1 AND memory_id = ?2",
        params![memory_type, memory_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
