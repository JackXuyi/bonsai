//! SQLite schema (v2) and primitives for the memory layer.
//!
//! Plan: docs/zh/plan-memory.md §3–§4.
//! Three layers: events (ground truth) → facts (traced assertions) →
//! preferences/boundaries (high-confidence user model).
//!
//! Invariants enforced here (plan §3):
//! - Traceability: facts carry source_event_id; boundaries carry source_fact_id.
//! - No silent rewrite: superseded rows get `invalid_at`, never UPDATEd content.
//! - Forgetting: user-requested deletes are physical and cascade (compliance
//!   Art. 16). `forget_event` / `forget_session` / `forget_all`.

use rusqlite::{params, Connection};

use super::clock;

pub(crate) const SCHEMA_VERSION: i64 = 2;

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Full schema for a fresh database (v2).
pub(crate) const SCHEMA: &str = r#"
-- Layer 1: raw events. The ground truth everything else traces back to.
CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    kind       TEXT NOT NULL,
    content    TEXT NOT NULL,
    role       TEXT,                        -- 'user' | 'assistant' | 'system'; NULL = non-dialogue event
    session_id TEXT,                        -- same dialogue window share one id
    consumed   INTEGER NOT NULL DEFAULT 0   -- consumed by a consolidate run
);

-- Layer 2: extracted facts. Superseded facts are stamped invalid_at (never
-- hard-deleted by the system; hard delete only on user request).
CREATE TABLE IF NOT EXISTS facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              TEXT NOT NULL,
    content         TEXT NOT NULL,
    source_event_id INTEGER REFERENCES events(id),
    invalid_at      TEXT,
    confidence      REAL NOT NULL DEFAULT 0.6,  -- rises on repeated mention, gate for preference promotion
    importance      INTEGER NOT NULL DEFAULT 3  -- 1..5; P0 keeps the column only (plan: no logic yet)
);

-- Layer 3a: user preferences (report §4.1: four categories).
CREATE TABLE IF NOT EXISTS preferences (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL,
    category   TEXT NOT NULL,               -- 'like' | 'habit' | 'context' | 'boundary'
    content    TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    invalid_at TEXT
);

-- Layer 3b: boundaries — "what it must never do/say". Most valuable kind:
-- respect > recording. Exempt from time decay in retrieval.
CREATE TABLE IF NOT EXISTS boundaries (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             TEXT NOT NULL,
    content        TEXT NOT NULL,
    source_fact_id INTEGER REFERENCES facts(id)
);

-- Consolidation audit (plan §4): what ran, over which window, what it did.
CREATE TABLE IF NOT EXISTS consolidate_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    trigger       TEXT NOT NULL,   -- 'nightly' | 'catchup' | 'manual'
    window_start  TEXT NOT NULL,
    window_end    TEXT NOT NULL,
    extracted     INTEGER NOT NULL DEFAULT 0,
    merged        INTEGER NOT NULL DEFAULT 0,
    contradicted  INTEGER NOT NULL DEFAULT 0,
    promoted      INTEGER NOT NULL DEFAULT 0,
    deleted       INTEGER NOT NULL DEFAULT 0,
    consumed      INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'running'  -- 'running' | 'done' | 'failed'
);

CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_unconsumed ON events(consumed, ts);
CREATE INDEX IF NOT EXISTS idx_facts_active ON facts(ts) WHERE invalid_at IS NULL;

-- TODO(vector): vector index for hybrid retrieval (embeddings + time decay).
-- No embedding column yet — schema migration waits for the sqlite-vec vs
-- JSON decision so v1→v2→v3 stay cheap (plan §7).
"#;

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn set_user_version(conn: &Connection, v: i64) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {v};"))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(());
        }
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))
}

/// Idempotent migrator. Handles:
/// - fresh DB → full v2 schema;
/// - v1 DB (scaffold-era, user_version 0 but tables exist) → in-place column
///   additions + audit table (plan §4, "v1 旧库打开 → 升级 → 旧数据无损").
pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version = user_version(conn)?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }

    let has_events: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'events'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)?;

    if !has_events {
        conn.execute_batch(SCHEMA)?;
    } else {
        // v1 → v2: additive only, existing rows keep their defaults.
        add_column_if_missing(conn, "events", "role", "TEXT")?;
        add_column_if_missing(conn, "events", "session_id", "TEXT")?;
        add_column_if_missing(conn, "events", "consumed", "INTEGER NOT NULL DEFAULT 0")?;
        add_column_if_missing(conn, "facts", "confidence", "REAL NOT NULL DEFAULT 0.6")?;
        add_column_if_missing(conn, "facts", "importance", "INTEGER NOT NULL DEFAULT 3")?;
        add_column_if_missing(conn, "boundaries", "source_fact_id", "INTEGER REFERENCES facts(id)")?;
        // SCHEMA uses IF NOT EXISTS everywhere, so re-running it only adds
        // missing tables (consolidate_runs) and indexes.
        conn.execute_batch(SCHEMA)?;
    }

    set_user_version(conn, SCHEMA_VERSION)?;
    Ok(())
}

// ---------------------------------------------------------------- row types

#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub ts: String,
    pub kind: String,
    pub role: Option<String>,
    pub session_id: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ActiveFact {
    pub id: i64,
    pub ts: String,
    pub content: String,
    pub confidence: f64,
    pub importance: i64,
    pub source_event_id: Option<i64>,
}

// ---------------------------------------------------------------- writes

/// Append a raw event (scaffold-compatible signature used by face.rs).
pub fn add_event(conn: &Connection, kind: &str, content: &str) -> rusqlite::Result<i64> {
    add_event_meta(conn, kind, content, None, None)
}

/// Append a raw event with dialogue metadata (role / session).
pub fn add_event_meta(
    conn: &Connection,
    kind: &str,
    content: &str,
    role: Option<&str>,
    session_id: Option<&str>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events (ts, kind, content, role, session_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![clock::now_iso(), kind, content, role, session_id],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn add_fact(conn: &Connection, content: &str, source_event_id: Option<i64>) -> rusqlite::Result<i64> {
    add_fact_confident(conn, content, source_event_id, 0.6)
}

pub fn add_fact_confident(
    conn: &Connection,
    content: &str,
    source_event_id: Option<i64>,
    confidence: f64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO facts (ts, content, source_event_id, confidence) VALUES (?1, ?2, ?3, ?4)",
        params![clock::now_iso(), content, source_event_id, confidence],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Quantize confidence to two decimals — `0.7 + 0.1` must land exactly on
/// the 0.8 promotion gate, not on f64's 0.7999999999999999.
fn quantize_conf(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Repeated mention: confidence rises by `delta`, capped at 0.95.
pub fn bump_fact(conn: &Connection, fact_id: i64, delta: f64) -> rusqlite::Result<()> {
    let current: f64 = conn.query_row(
        "SELECT confidence FROM facts WHERE id = ?1",
        params![fact_id],
        |r| r.get(0),
    )?;
    let next = quantize_conf((current + delta).min(0.95));
    conn.execute(
        "UPDATE facts SET confidence = ?1 WHERE id = ?2",
        params![next, fact_id],
    )?;
    Ok(())
}

/// Mark a fact as superseded (soft delete / contradiction resolution).
/// The old value is never rewritten — `invalid_at` preserves the
/// "what we used to believe" history (plan §3 invariant 2).
pub fn invalidate_fact(conn: &Connection, fact_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE facts SET invalid_at = ?1 WHERE id = ?2 AND invalid_at IS NULL",
        params![clock::now_iso(), fact_id],
    )
}

/// Boundary write: highest priority, bypasses confidence gates (plan §5.3).
pub fn add_boundary(
    conn: &Connection,
    content: &str,
    source_fact_id: Option<i64>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO boundaries (ts, content, source_fact_id) VALUES (?1, ?2, ?3)",
        params![clock::now_iso(), content, source_fact_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Normalized-equality guard so repeated boundary phrasings don't pile up.
pub fn boundary_exists(conn: &Connection, content: &str) -> rusqlite::Result<bool> {
    let norm = super::extract::normalize(content);
    let mut stmt = conn.prepare("SELECT content FROM boundaries")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        if super::extract::normalize(&row?) == norm {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Preference write (used by consolidate promotion, stage ⑤).
pub fn add_preference(
    conn: &Connection,
    category: &str,
    content: &str,
    confidence: f64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO preferences (ts, category, content, confidence) VALUES (?1, ?2, ?3, ?4)",
        params![clock::now_iso(), category, content, confidence],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Promote a fact only if no active preference holds the same content
/// (dedup guard — promotion is idempotent across nightly runs).
pub fn promote_fact(
    conn: &Connection,
    category: &str,
    content: &str,
    confidence: f64,
) -> rusqlite::Result<bool> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM preferences WHERE content = ?1 AND invalid_at IS NULL",
            params![content],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)?;
    if exists {
        return Ok(false);
    }
    add_preference(conn, category, content, confidence)?;
    Ok(true)
}

// --------------------------------------------------- forget (compliance Art. 16)

/// User: "forget this" → truly delete, cleanly: the event and every fact
/// derived from it.
pub fn forget_event(conn: &Connection, event_id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM facts WHERE source_event_id = ?1", params![event_id])?;
    conn.execute("DELETE FROM events WHERE id = ?1", params![event_id])
}

/// User: "forget that whole conversation" → cascade by session.
pub fn forget_session(conn: &Connection, session_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM facts WHERE source_event_id IN
             (SELECT id FROM events WHERE session_id = ?1)",
        params![session_id],
    )?;
    conn.execute("DELETE FROM events WHERE session_id = ?1", params![session_id])
}

/// User: "forget everything" → the nuclear option. Wipes all three layers.
pub fn forget_all(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM boundaries; DELETE FROM preferences; DELETE FROM facts; DELETE FROM events;",
    )?;
    Ok(())
}

// ------------------------------------------------------- reads for consolidate

/// Unconsumed events inside `[window_start, window_end)`, oldest first.
pub fn unconsumed_events(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, kind, role, session_id, content FROM events
         WHERE consumed = 0 AND ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![window_start, window_end], |row| {
        Ok(EventRow {
            id: row.get(0)?,
            ts: row.get(1)?,
            kind: row.get(2)?,
            role: row.get(3)?,
            session_id: row.get(4)?,
            content: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn mark_events_consumed(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE events SET consumed = 1 WHERE consumed = 0 AND ts >= ?1 AND ts < ?2",
        params![window_start, window_end],
    )
}

pub fn active_facts(conn: &Connection) -> rusqlite::Result<Vec<ActiveFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence, importance, source_event_id FROM facts
         WHERE invalid_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ActiveFact {
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            confidence: row.get(3)?,
            importance: row.get(4)?,
            source_event_id: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Facts whose ts is older than `iso_cutoff` and confidence below `max_conf`.
/// Used by stage ⑤ deletion (defensive: rule-extracted facts stay ≥ 0.5,
/// so this currently only sweeps legacy/manual low-confidence entries).
pub fn stale_low_confidence_facts(
    conn: &Connection,
    iso_cutoff: &str,
    max_conf: f64,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM facts
         WHERE invalid_at IS NULL AND confidence < ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![max_conf, iso_cutoff], |row| row.get(0))?;
    rows.collect()
}

// ---------------------------------------------------------- time-window reads

/// All events in an ISO-time window `[start, end)`, oldest first.
/// Feeds the daily report and the proactive-candidate value heuristic.
pub fn events_in_window(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, kind, role, session_id, content FROM events
         WHERE ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![start_iso, end_iso], |row| {
        Ok(EventRow {
            id: row.get(0)?,
            ts: row.get(1)?,
            kind: row.get(2)?,
            role: row.get(3)?,
            session_id: row.get(4)?,
            content: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Facts created in the window (regardless of validity — the report shows
/// what was *learned* that day, corrections included).
pub fn facts_in_window(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<Vec<ActiveFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence, importance, source_event_id FROM facts
         WHERE ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![start_iso, end_iso], |row| {
        Ok(ActiveFact {
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            confidence: row.get(3)?,
            importance: row.get(4)?,
            source_event_id: row.get(5)?,
        })
    })?;
    rows.collect()
}

// ------------------------------------------------- retrieval (M4: decayed hybrid)

/// Parse an RFC3339 ts; unparsable strings are treated as "fresh" (age 0).
fn age_days(ts: &str) -> f64 {
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso()).ok();
    let then = chrono::DateTime::parse_from_rfc3339(ts).ok();
    match (now, then) {
        (Some(n), Some(t)) => ((n - t).num_seconds() as f64 / 86_400.0).max(0.0),
        _ => 0.0,
    }
}

fn decay(age: f64) -> f64 {
    (-age / 45.0).exp() // plan §7 pins τ = 45 days
}

/// Keyword leg of hybrid retrieval across all three layers, with time-decay
/// scoring (plan §7). Boundaries and preferences are exempt from decay and
/// rank first; facts get importance × confidence × decay; events decay too.
/// `with_decayed = false` turns decay off ("翻旧账" mode for growth stories).
pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    with_decayed: bool,
) -> rusqlite::Result<Vec<super::MemoryHit>> {
    let pattern = format!("%{}%", query.replace('%', ""));
    let mut scored: Vec<(f64, super::MemoryHit)> = Vec::new();

    // Boundaries: constant top priority, never decayed.
    let mut stmt = conn.prepare(
        "SELECT id, ts, content FROM boundaries WHERE content LIKE ?1 ORDER BY id ASC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(super::MemoryHit {
            kind: "boundary".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: false,
        })
    })?;
    for row in rows {
        scored.push((1.0, row?));
    }

    // Preferences: constant, just below boundaries; invalidated ones sink.
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, invalid_at FROM preferences
         WHERE content LIKE ?1 ORDER BY (invalid_at IS NULL) DESC, ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        let hit = super::MemoryHit {
            kind: "preference".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: row.get::<_, Option<String>>(3)?.is_some(),
        };
        let score = if hit.invalidated { 0.1 } else { 0.9 };
        Ok((score, hit))
    })?;
    for row in rows {
        scored.push(row?);
    }

    // Facts: importance/3 × (0.5 + 0.5·confidence) × decay; invalidated facts
    // take a 0.1 penalty so a superseded value never outranks its successor
    // (v1 semantics, kept in v2 — see eval 误记率 metric).
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, invalid_at, confidence, importance FROM facts
         WHERE content LIKE ?1 ORDER BY ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        let hit = super::MemoryHit {
            kind: "fact".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: row.get::<_, Option<String>>(3)?.is_some(),
        };
        Ok((hit, row.get::<_, f64>(4)?, row.get::<_, i64>(5)?))
    })?;
    for row in rows {
        let (hit, confidence, importance) = row?;
        let score = (importance as f64 / 3.0)
            * (0.5 + 0.5 * confidence)
            * if with_decayed { decay(age_days(&hit.ts)) } else { 1.0 }
            * if hit.invalidated { 0.1 } else { 1.0 };
        scored.push((score, hit));
    }

    // Events: plain decay.
    let mut stmt = conn.prepare(
        "SELECT id, ts, content FROM events WHERE content LIKE ?1 ORDER BY ts DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(super::MemoryHit {
            kind: "event".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: false,
        })
    })?;
    for row in rows {
        let hit = row?;
        let score = 0.6 * if with_decayed { decay(age_days(&hit.ts)) } else { 1.0 };
        scored.push((score, hit));
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().map(|(_, hit)| hit).take(limit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn all_events(conn: &Connection) -> Vec<EventRow> {
        unconsumed_events(conn, "1970-01-01T00:00:00Z", "2999-01-01T00:00:00Z").unwrap()
    }

    #[test]
    fn event_fact_search_invalidate_forget() {
        let conn = mem();
        let ev = add_event(&conn, "chat", "user said they love rainy days").unwrap();
        let fact = add_fact(&conn, "user loves rainy days", Some(ev)).unwrap();

        let hits = search(&conn, "rainy", 10, true).unwrap();
        assert_eq!(hits.len(), 2); // fact first (higher score), then event

        invalidate_fact(&conn, fact).unwrap();
        let hits = search(&conn, "rainy", 10, true).unwrap();
        // Invalidated fact sinks (0.1 penalty): the active event ranks first,
        // the superseded fact trails.
        assert_eq!(hits[0].kind, "event");
        assert!(hits[1].invalidated);

        forget_event(&conn, ev).unwrap();
        let hits = search(&conn, "rainy", 10, true).unwrap();
        assert!(hits.is_empty()); // truly deleted — nothing left to trace
    }

    #[test]
    fn bump_rises_and_caps() {
        let conn = mem();
        let a = add_fact(&conn, "user loves rainy days", None).unwrap();
        bump_fact(&conn, a, 0.1).unwrap();
        bump_fact(&conn, a, 10.0).unwrap();
        let facts = active_facts(&conn).unwrap();
        assert!((facts[0].confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn v1_database_migrates_losslessly() {
        // Recreate exactly the scaffold-era v1 tables (no role/session/consumed,
        // no confidence/importance, no consolidate_runs), then migrate.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL);
             CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_event_id INTEGER REFERENCES events(id),
                 invalid_at TEXT);
             CREATE TABLE preferences (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 category TEXT NOT NULL, content TEXT NOT NULL,
                 confidence REAL NOT NULL DEFAULT 0.5, invalid_at TEXT);
             CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES ('2026-11-01T00:00:00Z', 'chat', 'legacy row')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        // Legacy row intact with new columns at defaults; second run is a no-op.
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "legacy row");
        migrate(&conn).unwrap();
    }

    #[test]
    fn forget_session_and_all_cascade() {
        let conn = mem();
        let s1 = add_event_meta(&conn, "chat", "alpha", Some("user"), Some("sess-1")).unwrap();
        add_fact(&conn, "fact from sess-1", Some(s1)).unwrap();
        let s2 = add_event_meta(&conn, "chat", "beta", Some("user"), Some("sess-2")).unwrap();
        add_fact(&conn, "fact from sess-2", Some(s2)).unwrap();
        add_boundary(&conn, "never interrupt", None).unwrap();

        forget_session(&conn, "sess-1").unwrap();
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-2"));

        forget_all(&conn).unwrap();
        assert!(all_events(&conn).is_empty());
        assert!(active_facts(&conn).unwrap().is_empty());
    }

    #[test]
    fn promotion_is_idempotent() {
        let conn = mem();
        assert!(promote_fact(&conn, "like", "user loves rainy days", 0.9).unwrap());
        assert!(!promote_fact(&conn, "like", "user loves rainy days", 0.9).unwrap());
    }

    #[test]
    fn pinned_clock_is_used_for_writes() {
        let _guard = clock::pinned("2026-11-18T00:00:00Z");
        let conn = mem();
        add_event(&conn, "chat", "x").unwrap();
        let rows = all_events(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ts, "2026-11-18T00:00:00Z");
    }
}
