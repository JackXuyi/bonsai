//! SQLite schema (v4) and primitives for the memory layer.
//!
//! Plans: docs/zh/plan-memory.md §3–§4, docs/zh/plan-predict.md §3.4,
//! docs/zh/plan-morph.md §3.2.
//! Three layers: events (ground truth) → facts (traced assertions) →
//! preferences/boundaries (high-confidence user model).
//!
//! Invariants enforced here (plan §3):
//! - Traceability: facts carry source_event_id; boundaries carry source_fact_id.
//! - No silent rewrite: superseded rows get `invalid_at`, never UPDATEd content.
//! - Forgetting: user-requested deletes are physical and cascade (compliance
//!   Art. 16). `forget_event` / `forget_session` / `forget_all`.

use chrono::Timelike;
use rusqlite::{params, Connection};

use super::clock;

pub(crate) const SCHEMA_VERSION: i64 = 4;

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

-- Predict-verify loop audit (plan-predict.md §3.4). One row per LOCAL day
-- max (UNIQUE(date)); the row is created on the first scheduling decision of
-- the day and later mutated only by ADDING feedback / asked_at — history is
-- never rewritten (plan invariant 1).
CREATE TABLE IF NOT EXISTS predictions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    date           TEXT NOT NULL UNIQUE,    -- local calendar day YYYY-MM-DD
    guess          TEXT NOT NULL,
    basis          TEXT NOT NULL,           -- JSON: [{"type":"preference"|"event","id":N}]
    ask_after_hour INTEGER NOT NULL,        -- intended delivery hour (local)
    asked_at       TEXT,                    -- actual delivery; NULL = not asked
    skip_reason    TEXT,                    -- 'no_candidate'|'busy'|'spoke_today'|'backoff'
    feedback       TEXT,                    -- 'hit'|'miss'|'shut_up' (NULL = no reply yet)
    feedback_at    TEXT,
    resolved_at    TEXT                     -- 48h window close (auto-resolve)
);

-- Form-line growth (plan-morph.md §3.2). Single-row state (id = 1):
-- born_at anchors the day count (backfilled from the earliest event so
-- existing users don't restart), stage only ever increases (invariant 1),
-- last_tick_date makes the daily advance idempotent, dormant_since is the
-- reversible "sleeping" marker (report §6.3 decision 1: no death, only rest).
CREATE TABLE IF NOT EXISTS growth (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    born_at        TEXT NOT NULL,
    stage          INTEGER NOT NULL DEFAULT 0,
    last_tick_date TEXT,
    dormant_since  TEXT
);

-- Growth audit: one row per stage unlock. notified_at makes the
-- window-open announcement idempotent (announced exactly once).
CREATE TABLE IF NOT EXISTS growth_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    stage       INTEGER NOT NULL,
    unlocked_at TEXT NOT NULL,
    notified_at TEXT,
    note        TEXT
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

/// User: "forget everything" → the nuclear option. Wipes all three layers
/// plus the prediction audit (plan-predict.md §7) and the growth tables
/// (plan-morph.md §7: compliance extends to growth too — starting over IS
/// the honest interpretation of "forget everything").
pub fn forget_all(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM growth_log; DELETE FROM growth; DELETE FROM predictions; DELETE FROM boundaries; DELETE FROM preferences; DELETE FROM facts; DELETE FROM events;",
    )?;
    Ok(())
}

// ------------------------------------------------------- predict-verify loop
// (docs/zh/plan-predict.md §3.4, §4)

#[derive(Debug, Clone)]
pub struct PredictionRow {
    pub id: i64,
    pub date: String,
    pub guess: String,
    pub basis: String,
    pub ask_after_hour: i64,
    pub asked_at: Option<String>,
    pub skip_reason: Option<String>,
    pub feedback: Option<String>,
    pub feedback_at: Option<String>,
    pub resolved_at: Option<String>,
}

const PREDICTION_COLS: &str =
    "id, date, guess, basis, ask_after_hour, asked_at, skip_reason, feedback, feedback_at, resolved_at";

fn row_prediction(row: &rusqlite::Row) -> rusqlite::Result<PredictionRow> {
    Ok(PredictionRow {
        id: row.get(0)?,
        date: row.get(1)?,
        guess: row.get(2)?,
        basis: row.get(3)?,
        ask_after_hour: row.get(4)?,
        asked_at: row.get(5)?,
        skip_reason: row.get(6)?,
        feedback: row.get(7)?,
        feedback_at: row.get(8)?,
        resolved_at: row.get(9)?,
    })
}

/// Today's row, if one exists (asked or skipped).
pub fn prediction_on_day(conn: &Connection, date: &str) -> rusqlite::Result<Option<PredictionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PREDICTION_COLS} FROM predictions WHERE date = ?1"
    ))?;
    let mut rows = stmt.query_map(params![date], row_prediction)?;
    rows.next().transpose()
}

/// Fetch a prediction row by id (feedback writeback needs the basis).
pub fn prediction_on_day_by_id(
    conn: &Connection,
    id: i64,
) -> rusqlite::Result<PredictionRow> {
    conn.query_row(
        &format!("SELECT {PREDICTION_COLS} FROM predictions WHERE id = ?1"),
        params![id],
        row_prediction,
    )
}

/// Insert today's ASKED prediction. `UNIQUE(date)` makes double-ask
/// impossible; callers resolve the existing row first.
pub fn record_prediction(
    conn: &Connection,
    date: &str,
    guess: &str,
    basis_json: &str,
    ask_after_hour: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(date) DO UPDATE SET
           guess = excluded.guess, basis = excluded.basis,
           ask_after_hour = excluded.ask_after_hour, asked_at = excluded.asked_at
         WHERE asked_at IS NULL",
        params![date, guess, basis_json, ask_after_hour, clock::now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Record a skip decision for today (overwrites previous skip reason while
/// nothing has been asked — e.g. busy earlier, no_candidate later).
pub fn record_skip(conn: &Connection, date: &str, reason: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO predictions (date, guess, basis, ask_after_hour, skip_reason)
         VALUES (?1, '', '[]', 0, ?2)
         ON CONFLICT(date) DO UPDATE SET skip_reason = excluded.skip_reason
         WHERE asked_at IS NULL",
        params![date, reason],
    )?;
    Ok(())
}

/// Apply user feedback. Fails (Ok(false)) when already resolved or the 48h
/// window has closed — late feedback doesn't rewrite history.
pub fn set_prediction_feedback(
    conn: &Connection,
    id: i64,
    feedback: &str,
) -> rusqlite::Result<bool> {
    let (asked_at, existing_feedback, resolved_at): (
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn.query_row(
        "SELECT asked_at, feedback, resolved_at FROM predictions WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let Some(asked_at) = asked_at else {
        return Ok(false); // never asked
    };
    if existing_feedback.is_some() || resolved_at.is_some() {
        return Ok(false); // already has feedback or resolved
    }
    let Some(asked) = chrono::DateTime::parse_from_rfc3339(&asked_at).ok() else {
        return Ok(false);
    };
    // "now" goes through the injectable clock so the 48h window is
    // deterministic under tests.
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso())
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    if now - asked.with_timezone(&chrono::Utc) > chrono::Duration::hours(48) {
        return Ok(false);
    }
    let now_iso = clock::now_iso();
    let n = conn.execute(
        "UPDATE predictions SET feedback = ?2, feedback_at = ?3, resolved_at = ?3 WHERE id = ?1",
        params![id, feedback, now_iso],
    )?;
    Ok(n > 0)
}

/// Close the 48h window on stale unanswered predictions (weak negative
/// sample for the back-off-rate metric, plan §4). Called from the poll loop.
pub fn resolve_stale_predictions(conn: &Connection) -> rusqlite::Result<usize> {
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso())
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let cutoff = (now - chrono::Duration::hours(48)).to_rfc3339();
    conn.execute(
        "UPDATE predictions SET resolved_at = ?1
         WHERE asked_at IS NOT NULL AND feedback IS NULL AND resolved_at IS NULL
           AND asked_at < ?2",
        params![clock::now_iso(), cutoff],
    )
}

/// Active habits, highest confidence first — the predictor's evidence source.
pub fn active_habits(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, confidence FROM preferences
         WHERE category = 'habit' AND invalid_at IS NULL
         ORDER BY confidence DESC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

/// Quantized preference confidence bump (same two-decimal rule as facts;
/// clamped to [0.3, 0.95] — plan-predict.md §4).
pub fn bump_preference(conn: &Connection, pref_id: i64, delta: f64) -> rusqlite::Result<()> {
    let current: f64 = conn.query_row(
        "SELECT confidence FROM preferences WHERE id = ?1",
        params![pref_id],
        |r| r.get(0),
    )?;
    let next = quantize_conf((current + delta).clamp(0.3, 0.95));
    conn.execute(
        "UPDATE preferences SET confidence = ?1 WHERE id = ?2",
        params![next, pref_id],
    )?;
    Ok(())
}

/// One cell of the usage rhythm: how many distinct days did `app` show up
/// around `hour` (local time) over the look-back window, plus one sample
/// event id for basis traceability (plan-predict.md: explainable guesses).
#[derive(Debug, Clone)]
pub struct RhythmEntry {
    pub app: String,
    pub hour: u32,
    pub days: usize,
    pub sample_event_id: i64,
}

/// Days-per-(app, local-hour) histogram over the last `days` days, from
/// perception events of the shape "app: X". Feeds rule ② of RulePredictor.
pub fn app_rhythm(conn: &Connection, days: i64) -> rusqlite::Result<Vec<RhythmEntry>> {
    use std::collections::BTreeMap;
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, ts, content FROM events
         WHERE kind = 'perception' AND content LIKE 'app: %' AND ts >= ?1 ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    // (app, hour) -> distinct local dates; first-seen event id kept.
    let mut seen: BTreeMap<(String, u32), (std::collections::BTreeSet<String>, i64)> =
        BTreeMap::new();
    for row in rows {
        let (id, ts, content) = row?;
        let app = content.trim_start_matches("app: ").trim().to_string();
        let parsed = chrono::DateTime::parse_from_rfc3339(&ts)
            .map(|d| d.with_timezone(&chrono::Local));
        let (hour, date) = match parsed {
            Ok(local) => (local.hour(), local.format("%Y-%m-%d").to_string()),
            Err(_) => continue,
        };
        let entry = seen.entry((app, hour)).or_insert_with(|| (Default::default(), id));
        entry.0.insert(date);
    }
    Ok(seen
        .into_iter()
        .map(|((app, hour), (dates, sample))| RhythmEntry {
            app,
            hour,
            days: dates.len(),
            sample_event_id: sample,
        })
        .collect())
}

/// Did the proactive dialogue channel already speak today (mutual exclusion,
/// plan §3.3: a day that had its proactive message gets no prediction)?
pub fn proactive_spoke_today(conn: &Connection) -> rusqlite::Result<bool> {
    let (start, end) = super::super::report::local_day_window(chrono::Local::now().date_naive())
        .unwrap_or_default();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events
         WHERE role = 'assistant' AND session_id = 'proactive' AND ts >= ?1 AND ts < ?2",
        params![start, end],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Aggregate stats over resolved predictions (plan §4 metrics).
#[derive(Debug, Default, serde::Serialize)]
pub struct PredictionStats {
    pub hit: usize,
    pub miss: usize,
    pub shut_up: usize,
    pub no_reply: usize,
    pub asked: usize,
    /// hit / (hit + miss) over ALL resolved-with-verdict rows.
    pub accuracy: Option<f64>,
    /// hit / (hit + miss) over the last 7 resolved days, `None` if no verdicts.
    pub accuracy_7d: Option<f64>,
    /// shut_up / asked, `None` when nothing asked.
    pub backoff_rate: Option<f64>,
}

pub fn prediction_stats(conn: &Connection) -> rusqlite::Result<PredictionStats> {
    let mut s = PredictionStats::default();
    let mut stmt = conn.prepare(
        "SELECT feedback, asked_at, resolved_at FROM predictions WHERE asked_at IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut verdict_7d: (usize, usize) = (0, 0); // (hit, miss) within 7 days
    for row in rows {
        let (feedback, asked_at, resolved_at) = row?;
        s.asked += 1;
        match feedback.as_deref() {
            Some("hit") => s.hit += 1,
            Some("miss") => s.miss += 1,
            Some("shut_up") => s.shut_up += 1,
            _ => {}
        }
        if resolved_at.is_none() && feedback.is_none() {
            s.no_reply += 1;
        }
        let recent = chrono::DateTime::parse_from_rfc3339(&asked_at)
            .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) <= chrono::Duration::days(7))
            .unwrap_or(false);
        if recent {
            match feedback.as_deref() {
                Some("hit") => verdict_7d.0 += 1,
                Some("miss") => verdict_7d.1 += 1,
                _ => {}
            }
        }
    }
    let total_verdicts = s.hit + s.miss;
    if total_verdicts > 0 {
        s.accuracy = Some(s.hit as f64 / total_verdicts as f64);
    }
    let v7 = verdict_7d.0 + verdict_7d.1;
    if v7 > 0 {
        s.accuracy_7d = Some(verdict_7d.0 as f64 / v7 as f64);
    }
    if s.asked > 0 {
        s.backoff_rate = Some(s.shut_up as f64 / s.asked as f64);
    }
    Ok(s)
}

// ------------------------------------------------- form-line growth
// (docs/zh/plan-morph.md §3.2)

#[derive(Debug, Clone)]
pub struct GrowthState {
    pub born_at: String,
    pub stage: i64,
    pub last_tick_date: Option<String>,
    pub dormant_since: Option<String>,
}

/// The single growth row, if it exists yet.
pub fn growth_state(conn: &Connection) -> rusqlite::Result<Option<GrowthState>> {
    let mut stmt = conn.prepare(
        "SELECT born_at, stage, last_tick_date, dormant_since FROM growth WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(GrowthState {
            born_at: row.get(0)?,
            stage: row.get(1)?,
            last_tick_date: row.get(2)?,
            dormant_since: row.get(3)?,
        })
    })?;
    rows.next().transpose()
}

/// Ensure the growth row exists, anchoring `born_at` to the earliest event
/// (plan open question 3: existing users don't restart the clock — "it has
/// actually been with you for N days" is the stronger first impression).
/// The anchor is written once and never changed afterwards.
pub fn ensure_growth(conn: &Connection) -> rusqlite::Result<GrowthState> {
    if let Some(s) = growth_state(conn)? {
        return Ok(s);
    }
    let earliest: Option<String> = conn
        .query_row("SELECT MIN(ts) FROM events", [], |r| r.get(0))
        .ok()
        .flatten();
    let born_at = earliest.unwrap_or_else(|| clock::now_iso());
    conn.execute(
        "INSERT INTO growth (id, born_at, stage) VALUES (1, ?1, 0)",
        params![born_at],
    )?;
    Ok(GrowthState {
        born_at,
        stage: 0,
        last_tick_date: None,
        dormant_since: None,
    })
}

pub fn set_growth_stage(conn: &Connection, stage: i64) -> rusqlite::Result<()> {
    // Guarded by `stage > ?2` at the call site; the WHERE re-asserts the
    // monotonicity invariant (stage only ever increases).
    conn.execute(
        "UPDATE growth SET stage = ?1 WHERE id = 1 AND stage < ?1",
        params![stage],
    )?;
    Ok(())
}

pub fn set_growth_tick_date(conn: &Connection, date: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE growth SET last_tick_date = ?1 WHERE id = 1",
        params![date],
    )?;
    Ok(())
}

/// `Some(iso)` = went dormant; `None` = woke up. Never overwrites an
/// existing dormant_since with another one (first sleep wins until wake).
pub fn set_dormant(conn: &Connection, since: Option<&str>) -> rusqlite::Result<usize> {
    match since {
        Some(iso) => conn.execute(
            "UPDATE growth SET dormant_since = ?1 WHERE id = 1 AND dormant_since IS NULL",
            params![iso],
        ),
        None => conn.execute(
            "UPDATE growth SET dormant_since = NULL WHERE id = 1 AND dormant_since IS NOT NULL",
            [],
        ),
    }
}

pub fn add_growth_log(conn: &Connection, stage: i64, note: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO growth_log (stage, unlocked_at, note) VALUES (?1, ?2, ?3)",
        params![stage, clock::now_iso(), note],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Un-announced unlocks (oldest first) — the window-open announcement queue.
pub fn pending_growth_logs(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, stage, note FROM growth_log
         WHERE notified_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
    })?;
    rows.collect()
}

/// Idempotent announcement: only a row with notified_at still NULL is
/// marked, so a double window-open cannot announce twice.
pub fn mark_growth_notified(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE growth_log SET notified_at = ?1 WHERE id = ?2 AND notified_at IS NULL",
        params![clock::now_iso(), id],
    )?;
    Ok(n > 0)
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

/// Active user boundaries (prompt-injection source, plan-persona §3.2
/// 双源对齐: card boundaries + user-collected boundaries merged at render).
pub fn boundaries(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT content FROM boundaries ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// Last `max_turns` dialogue turns for a session, oldest first. Dialogue
/// events carry kind="dialogue", role user/assistant, and a session id.
pub fn dialogue_history(
    conn: &Connection,
    session_id: &str,
    max_turns: usize,
) -> rusqlite::Result<Vec<crate::persona::eval::Turn>> {
    let mut stmt = conn.prepare(
        "SELECT role, content FROM events
         WHERE session_id = ?1 AND kind = 'dialogue'
           AND role IN ('user', 'assistant')
         ORDER BY id DESC LIMIT ?2",
    )?;
    let mut rows: Vec<(String, String)> = stmt
        .query_map(params![session_id, (max_turns * 2) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?;
    rows.reverse(); // oldest first

    // Pair consecutive user → assistant rows into turns. An assistant row
    // without a preceding user row is a proactive message — kept, with a
    // synthetic user marker, so later chats still see what the companion
    // said on its own. A user row with no assistant reply is dropped (the
    // turn scrolled out of the limit or the model died mid-turn).
    let mut out = Vec::new();
    let mut pending_user: Option<String> = None;
    for (role, content) in rows {
        match role.as_str() {
            "user" => pending_user = Some(content),
            "assistant" => {
                let user =
                    pending_user.take().unwrap_or_else(|| "(盆栽主动说的)".to_string());
                out.push(crate::persona::eval::Turn { user, assistant: content });
            }
            _ => {}
        }
    }
    Ok(out)
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
    fn growth_anchor_backfills_from_earliest_event_and_never_moves() {
        let conn = mem();
        let _clock = clock::pinned("2026-11-01T00:00:00Z");
        add_event(&conn, "chat", "oldest").unwrap();
        let _clock2 = clock::pinned("2026-11-18T00:00:00Z");
        add_event(&conn, "chat", "newer").unwrap();

        let s = ensure_growth(&conn).unwrap();
        assert_eq!(s.born_at, "2026-11-01T00:00:00Z", "anchor = earliest event");
        assert_eq!(s.stage, 0);

        // The anchor is written once; later calls never move it.
        let s2 = ensure_growth(&conn).unwrap();
        assert_eq!(s2.born_at, s.born_at);
    }

    #[test]
    fn growth_stage_monotonic_and_announcement_idempotent() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 2).unwrap();
        // A smaller stage must not rewind the state (invariant 1).
        set_growth_stage(&conn, 0).unwrap();
        assert_eq!(growth_state(&conn).unwrap().unwrap().stage, 2);

        let id = add_growth_log(&conn, 2, "成形了。").unwrap();
        assert_eq!(pending_growth_logs(&conn).unwrap().len(), 1);
        assert!(mark_growth_notified(&conn, id).unwrap());
        // Second window open: the row is already announced → no-op.
        assert!(!mark_growth_notified(&conn, id).unwrap());
        assert!(pending_growth_logs(&conn).unwrap().is_empty());
    }

    #[test]
    fn dormancy_flips_are_guards_against_double_write() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        assert_eq!(set_dormant(&conn, Some("2026-11-19T00:00:00Z")).unwrap(), 1);
        // Already dormant: the second sleep write is a no-op.
        assert_eq!(set_dormant(&conn, Some("2026-11-20T00:00:00Z")).unwrap(), 0);
        let s = growth_state(&conn).unwrap().unwrap();
        assert_eq!(s.dormant_since.as_deref(), Some("2026-11-19T00:00:00Z"));
        assert_eq!(set_dormant(&conn, None).unwrap(), 1);
        // Awake: a second wake is a no-op.
        assert_eq!(set_dormant(&conn, None).unwrap(), 0);
    }

    #[test]
    fn forget_all_cascades_growth_tables() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 1).unwrap();
        add_growth_log(&conn, 1, "抽枝了。").unwrap();
        forget_all(&conn).unwrap();
        assert!(growth_state(&conn).unwrap().is_none());
        // A later ensure re-anchors from scratch (events are gone too).
        let s = ensure_growth(&conn).unwrap();
        assert_eq!(s.stage, 0);
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
