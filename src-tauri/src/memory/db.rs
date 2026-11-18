//! SQLite schema and primitives for the memory layer.

use rusqlite::{params, Connection};

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub(crate) const SCHEMA: &str = r#"
-- Raw events: the ground truth everything else must trace back to (explainability).
CREATE TABLE IF NOT EXISTS events (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts      TEXT NOT NULL,
    kind    TEXT NOT NULL,
    content TEXT NOT NULL
);

-- Extracted facts: consolidated knowledge. Superseded facts are stamped
-- invalid_at (never hard-deleted by the system; hard delete only on user request
-- — "forget this" must truly delete, see forget_user_data()).
CREATE TABLE IF NOT EXISTS facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              TEXT NOT NULL,
    content         TEXT NOT NULL,
    source_event_id INTEGER REFERENCES events(id),
    invalid_at      TEXT
);

-- User preferences (report §4.1: preferences are split into four types;
-- boundaries are their own table, not a preference category).
CREATE TABLE IF NOT EXISTS preferences (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    category   TEXT NOT NULL,
    content    TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    invalid_at TEXT
);

-- Boundaries: "what it must never do/say" — the most valuable preference type
-- (respect > recording).
CREATE TABLE IF NOT EXISTS boundaries (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts      TEXT NOT NULL,
    content TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_facts_active ON facts(ts) WHERE invalid_at IS NULL;

-- TODO(vector): vector index for hybrid retrieval (embeddings + time decay).
"#;

pub fn add_event(conn: &Connection, kind: &str, content: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events (ts, kind, content) VALUES (?1, ?2, ?3)",
        params![now(), kind, content],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn add_fact(
    conn: &Connection,
    content: &str,
    source_event_id: Option<i64>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO facts (ts, content, source_event_id) VALUES (?1, ?2, ?3)",
        params![now(), content, source_event_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Mark a fact as superseded (soft delete / contradiction resolution).
pub fn invalidate_fact(conn: &Connection, fact_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE facts SET invalid_at = ?1 WHERE id = ?2 AND invalid_at IS NULL",
        params![now(), fact_id],
    )
}

/// Compliance path: user says "forget this" → truly delete, cleanly.
/// (Report §4.2: forgettable is a hard requirement, not a nice-to-have.)
pub fn forget_event(conn: &Connection, event_id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM facts WHERE source_event_id = ?1", params![event_id])?;
    conn.execute("DELETE FROM events WHERE id = ?1", params![event_id])
}

/// Keyword leg of hybrid retrieval. Active facts first, then events.
pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<super::MemoryHit>> {
    let pattern = format!("%{}%", query.replace('%', ""));
    let mut hits = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT id, ts, content, invalid_at FROM facts
         WHERE content LIKE ?1 ORDER BY (invalid_at IS NULL) DESC, ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(super::MemoryHit {
            kind: "fact".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: row.get::<_, Option<String>>(3)?.is_some(),
        })
    })?;
    for row in rows {
        hits.push(row?);
    }

    let events_left = limit.saturating_sub(hits.len());
    if events_left > 0 {
        let mut stmt = conn.prepare(
            "SELECT id, ts, content FROM events WHERE content LIKE ?1 ORDER BY ts DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, events_left as i64], |row| {
            Ok(super::MemoryHit {
                kind: "event".into(),
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                invalidated: false,
            })
        })?;
        for row in rows {
            hits.push(row?);
        }
    }

    Ok(hits)
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[test]
    fn event_fact_search_invalidate_forget() {
        let conn = mem();
        let ev = add_event(&conn, "chat", "user said they love rainy days").unwrap();
        let fact = add_fact(&conn, "user loves rainy days", Some(ev)).unwrap();

        let hits = search(&conn, "rainy", 10).unwrap();
        assert_eq!(hits.len(), 2); // fact first, then event

        invalidate_fact(&conn, fact).unwrap();
        let hits = search(&conn, "rainy", 10).unwrap();
        assert!(hits[0].invalidated);

        forget_event(&conn, ev).unwrap();
        let hits = search(&conn, "rainy", 10).unwrap();
        assert!(hits.is_empty()); // truly deleted — nothing left to trace
    }
}
