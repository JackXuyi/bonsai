//! SQLite schema (v9) and primitives for the memory layer.
//!
//! Plans: docs/zh/plan-memory.md §3–§4, docs/zh/plan-predict.md §3.4,
//! docs/zh/plan-morph.md §3.2, docs/zh/plan-onboarding.md §3.1,
//! docs/zh/plan-relationship.md §3.3, docs/zh/plan-calendar.md §3.2.
//! Three layers: events (ground truth) → facts (traced assertions) →
//! preferences/boundaries (high-confidence user model).
//!
//! Invariants enforced here (plan §3):
//! - Traceability: facts carry source_event_id; boundaries carry source_fact_id.
//! - No silent rewrite: superseded rows get `invalid_at`, never UPDATEd content.
//! - Forgetting: user-requested deletes are physical and cascade (compliance
//!   Art. 16). `forget_event` / `forget_session` / `forget_all`.

use chrono::Timelike;
use rusqlite::{params, Connection, OptionalExtension};

use super::clock;

/// SCHEMA_VERSION = 11（2026-11-21）：trace_steps 补 user_text 列——当前
/// 输入作为 user message 传给模型（不在 system prompt 里），step 级 trace
/// 此前看不到它（turn 级 turn_trace.user_text 一直有）。
/// SCHEMA_VERSION = 10（2026-11-21）：turn_trace 补 user_text/reply_text 列。
/// 上一版 v9 build 已在旧库建过 turn_trace（无这两列），CREATE TABLE IF NOT
/// EXISTS 不会给已有表加列；v9→v9 不触发 migrate，INSERT 带 user_text 直接
/// 报 "table turn_trace has no column named user_text"。教训：改 SCHEMA 必
/// 须同时 bump 此版本号。
pub(crate) const SCHEMA_VERSION: i64 = 11;

/// Test-only: materialise the full schema on a bare in-memory connection.
/// Other modules' tests build their own `Connection::open_in_memory()` and
/// need the schema without going through `open()` (which wants a path).
#[cfg(test)]
pub fn init_schema_for_test(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

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

-- Onboarding (plan-onboarding.md §3.1). `settings` is a tiny KV for
-- app-level state (onboarding_done / age_group / crisis_ack). The answers
-- table is the audit trail — SKIPPED answers are recorded too (skipped=1
-- is itself a signal for tuning the questions later).
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS onboarding_answers (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    question TEXT NOT NULL,                 -- 'q1'..'q5' | 'age' | 'crisis'
    answer   TEXT NOT NULL,
    skipped  INTEGER NOT NULL DEFAULT 0,    -- 1 = user skipped
    ts       TEXT NOT NULL
);

-- Relationship line (plan-relationship.md §3.3). Single-row state (id = 1).
-- No born_at here: the day count reuses growth.born_at so the two timelines
-- share one origin. stage only ever increases; cold_since is the reversible
-- "rusty" marker (never rewinds the stage).
CREATE TABLE IF NOT EXISTS relationship (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    stage          INTEGER NOT NULL DEFAULT 0,   -- 0 拘谨 | 1 熟络 | 2 会顶嘴
    stage_since    TEXT,                          -- current stage start (7-day "first refusal" window)
    cold_since     TEXT,                          -- NULL = warm
    last_tick_date TEXT                           -- makes the daily advance idempotent
);

-- Daily evidence, RECOMPUTED not accumulated: one UPSERT per day derived from
-- predictions + events. Same source data → same score (replayable, testable).
-- The numeric score is internal only — it never leaves this process.
CREATE TABLE IF NOT EXISTS relationship_progress (
    date          TEXT PRIMARY KEY,               -- local day YYYY-MM-DD
    present       INTEGER NOT NULL DEFAULT 0,
    hits          INTEGER NOT NULL DEFAULT 0,
    misses        INTEGER NOT NULL DEFAULT 0,
    disagreements INTEGER NOT NULL DEFAULT 0,
    shut_ups      INTEGER NOT NULL DEFAULT 0,
    score         INTEGER NOT NULL DEFAULT 0,
    computed_at   TEXT NOT NULL
);

-- Relationship audit: one row per stage unlock (notified_at = announced once).
-- `kind` separates stage promotions from the one-off "first refusal" moment.
CREATE TABLE IF NOT EXISTS relationship_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL DEFAULT 'stage',   -- 'stage' | 'first_refusal'
    stage       INTEGER NOT NULL,
    unlocked_at TEXT NOT NULL,
    notified_at TEXT,
    note        TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_unconsumed ON events(consumed, ts);
CREATE INDEX IF NOT EXISTS idx_facts_active ON facts(ts) WHERE invalid_at IS NULL;

-- Calendar read cache (plan-calendar.md §3.2). PURE CACHE: whole table is
-- disposable — TTL 5 min, cleared on switch-off / permission loss / forget.
-- Titles live here and ONLY here: they never reach events or facts.
CREATE TABLE IF NOT EXISTS calendar_cache (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id    TEXT NOT NULL,                -- EKEvent.eventIdentifier
    start_at       TEXT NOT NULL,                -- RFC3339 UTC (same clock as events.ts)
    end_at         TEXT NOT NULL,
    title          TEXT NOT NULL,
    calendar_name  TEXT,
    all_day        INTEGER NOT NULL DEFAULT 0,
    attendee_count INTEGER NOT NULL DEFAULT 0,
    declined       INTEGER NOT NULL DEFAULT 0,   -- the current user declined it
    canceled       INTEGER NOT NULL DEFAULT 0,
    availability   TEXT,                         -- 'busy' | 'free' | 'tentative' | 'unavailable'
    reminded_at    TEXT,                         -- one reminder per occurrence
    fetched_at     TEXT NOT NULL,
    -- Recurring instances share an eventIdentifier, so the start time is part
    -- of the identity — without it every occurrence overwrites the last one.
    UNIQUE(external_id, start_at)
);

-- "What did it do" audit (plan-calendar.md §3.2): action + outcome + timing
-- only. No titles, no bodies — this table is safe to show the user.
CREATE TABLE IF NOT EXISTS tool_calls (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL,
    tool        TEXT NOT NULL,     -- 'calendar' | 'mail' | 'mcp' | 'skill'
    action      TEXT NOT NULL,     -- 'agenda' | 'next' | 'search' | 'remind' | 'digest' | <descriptor name>
    outcome     TEXT NOT NULL,     -- 'ok' | 'denied' | 'unavailable' | 'error'
    detail      TEXT,              -- counts / latency, never content
    duration_ms INTEGER,
    -- agent-pipeline audit columns (spec-s4; v7→v8 upgrades get them via
    -- add_column_if_missing — fresh DBs get them here).
    params_summary TEXT,
    result_status  TEXT,
    prompt_trace   TEXT,
    remote         INTEGER DEFAULT 0
);

-- MCP servers (plan-agent-mcp-skill.md §3.2, spec-s2). Secrets in
-- headers_json/env_json are plaintext at rest (local SQLite, same policy as
-- cloud keys); every read path handed to the frontend must mask them.
CREATE TABLE IF NOT EXISTS mcp_servers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    transport     TEXT NOT NULL DEFAULT 'stdio',  -- 'stdio' | 'http'
    command       TEXT,                           -- stdio: executable path (NULL for http)
    args_json     TEXT NOT NULL DEFAULT '[]',
    url           TEXT,                           -- http: remote endpoint (NULL for stdio)
    headers_json  TEXT NOT NULL DEFAULT '{}',
    env_json      TEXT NOT NULL DEFAULT '{}',
    enabled       INTEGER NOT NULL DEFAULT 0,     -- new servers start disabled (RK-04)
    tool_cache    TEXT,
    tool_cache_at TEXT,
    created_at    TEXT NOT NULL
);

-- Turn trace (spec-s8): how one reply was produced. Sidecar record —
-- NEVER part of ACTIVITY_KINDS and never feeds morph/relationship/growth:
-- tracing our own work must not count as "being talked to".
-- D1 (2026-11-20): prompts/outputs stored FULL TEXT, no pruning, no TTL —
-- the trace is a debugging & trust product; truncating it destroys the
-- evidence. Storage cost is accepted on local SQLite.
CREATE TABLE IF NOT EXISTS turn_trace (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id         TEXT NOT NULL,
    user_event_id      INTEGER,        -- events.id of the user side (NULL for proactive)
    assistant_event_id INTEGER,        -- events.id of the assistant side
    kind               TEXT NOT NULL,  -- 'model' | 'tool' | 'crisis' | 'proactive'
    model_used         INTEGER NOT NULL DEFAULT 0,
    tool_label         TEXT,           -- 'builtin:x' | 'mcp:x' | 'skill:x' | 'calendar:action'
    filter_report      TEXT,           -- JSON: {"unconditional_affirmation","hedged","explicit"}
    duration_ms        INTEGER,
    user_text          TEXT,           -- 用户原文（摘要卡显示；proactive 为 NULL）
    reply_text         TEXT,           -- 过滤后的最终回复（摘要卡显示）
    ts                 TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turn_trace_user ON turn_trace(user_event_id);
CREATE INDEX IF NOT EXISTS idx_turn_trace_asst ON turn_trace(assistant_event_id);

-- One row per pipeline step inside a turn (spec-s8 §3.2). seq orders the
-- timeline; step_kind drives the frontend's node coloring.
CREATE TABLE IF NOT EXISTS trace_steps (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id     INTEGER NOT NULL REFERENCES turn_trace(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    step_kind   TEXT NOT NULL,   -- crisis_check|tool_intent|llm_round|tool_call|json_retry|prose_retry|filter
    title       TEXT NOT NULL,   -- display heading (Chinese, user-facing)
    prompt      TEXT,            -- llm_round: full system prompt of THIS round (D1: unclipped)
    output      TEXT,            -- llm_round: raw model output; tool_call: reply text / error
    args_json   TEXT,            -- tool_call: args JSON verbatim
    summary     TEXT,            -- tool_call: ToolOutput.summary
    outcome     TEXT,            -- ok|denied|error|hit|miss|pass|answered|direct|none
    duration_ms INTEGER,
    user_text   TEXT,           -- v11: the current user message sent THIS round
    ts          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trace_steps_turn ON trace_steps(turn_id, seq);

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
        // missing tables (consolidate_runs, mcp_servers) and indexes.
        conn.execute_batch(SCHEMA)?;
        // v7 → v8: audit columns for the agent tool pipeline (spec-s2).
        add_column_if_missing(conn, "tool_calls", "params_summary", "TEXT")?;
        add_column_if_missing(conn, "tool_calls", "result_status", "TEXT")?;
        add_column_if_missing(conn, "tool_calls", "prompt_trace", "TEXT")?;
        add_column_if_missing(conn, "tool_calls", "remote", "INTEGER DEFAULT 0")?;
        // v9 trace tables + summary columns (spec-s8; additive on DBs that
        // already created the tables in an earlier v9 build).
        add_column_if_missing(conn, "turn_trace", "user_text", "TEXT")?;
        add_column_if_missing(conn, "turn_trace", "reply_text", "TEXT")?;
        // v11: step 级当前输入（user message 不在 system prompt 里，trace 盲点）
        add_column_if_missing(conn, "trace_steps", "user_text", "TEXT")?;
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
        "DELETE FROM tool_calls; DELETE FROM calendar_cache; DELETE FROM relationship_log; DELETE FROM relationship_progress; DELETE FROM relationship; DELETE FROM onboarding_answers; DELETE FROM settings; DELETE FROM growth_log; DELETE FROM growth; DELETE FROM predictions; DELETE FROM boundaries; DELETE FROM preferences; DELETE FROM facts; DELETE FROM events;",
    )?;
    Ok(())
}

/// Reset ONLY the relationship line (tool-page button, plan §3.8): the user
/// may want to "start over" socially without losing memory or the plant.
pub fn forget_relationship(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM relationship_log; DELETE FROM relationship_progress; DELETE FROM relationship;",
    )?;
    Ok(())
}

// ----------------------------------------------- settings + onboarding audit
// (docs/zh/plan-onboarding.md §3.1)

/// Read a settings key. Missing key → None (this is how "old user without
/// onboarding_done" is detected: they get the flow on next window open).
pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Upsert a settings key.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, clock::now_iso()],
    )?;
    Ok(())
}

/// Clear a settings key (the tray "redo onboarding" path: keep the answers,
/// drop only the done flag).
pub fn remove_setting(conn: &Connection, key: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
}

/// One onboarding answer, audit form (skipped answers included).
pub fn record_onboarding_answer(
    conn: &Connection,
    question: &str,
    answer: &str,
    skipped: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO onboarding_answers (question, answer, skipped, ts) VALUES (?1, ?2, ?3, ?4)",
        params![question, answer, skipped as i64, clock::now_iso()],
    )?;
    Ok(())
}

/// All recorded answers, oldest first.
pub fn onboarding_answers(conn: &Connection) -> rusqlite::Result<Vec<(String, String, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT question, answer, skipped FROM onboarding_answers ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
        ))
    })?;
    rows.collect()
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
pub fn proactive_spoke_today(conn: &Connection, day: chrono::NaiveDate) -> rusqlite::Result<bool> {
    // 「今天」必须由调用方传入（predict 传 schedule_iteration 的 now）：测试
    // 用 clock::pinned 钉住时间线时，真实 Local::now() 会查出空窗口导致误判。
    let (start, end) = super::super::report::local_day_window(day).unwrap_or_default();
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

// ------------------------------------------------- relationship (plan §3.3)

#[derive(Debug, Clone)]
pub struct RelationshipState {
    pub stage: i64,
    pub stage_since: Option<String>,
    pub cold_since: Option<String>,
    pub last_tick_date: Option<String>,
}

/// One day of understanding evidence (plan §3.2). `score` is derived by
/// `relationship::score_day` — it exists here only as an audit column and is
/// NEVER returned to the frontend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayEvidence {
    pub present: i64,
    pub hits: i64,
    pub misses: i64,
    pub disagreements: i64,
    pub shut_ups: i64,
    pub score: i64,
}

pub fn relationship_state(conn: &Connection) -> rusqlite::Result<Option<RelationshipState>> {
    let mut stmt = conn.prepare(
        "SELECT stage, stage_since, cold_since, last_tick_date FROM relationship WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(RelationshipState {
            stage: row.get(0)?,
            stage_since: row.get(1)?,
            cold_since: row.get(2)?,
            last_tick_date: row.get(3)?,
        })
    })?;
    rows.next().transpose()
}

/// Ensure the single row exists. Unlike `ensure_growth` there is no anchor to
/// backfill — the relationship timeline reuses `growth.born_at`.
pub fn ensure_relationship(conn: &Connection) -> rusqlite::Result<RelationshipState> {
    if let Some(s) = relationship_state(conn)? {
        return Ok(s);
    }
    conn.execute("INSERT OR IGNORE INTO relationship (id, stage) VALUES (1, 0)", [])?;
    Ok(RelationshipState {
        stage: 0,
        stage_since: None,
        cold_since: None,
        last_tick_date: None,
    })
}

/// Monotonic stage write (the WHERE re-asserts invariant 1: never rewinds).
pub fn set_relationship_stage(conn: &Connection, stage: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE relationship SET stage = ?1, stage_since = ?2 WHERE id = 1 AND stage < ?1",
        params![stage, clock::now_iso()],
    )?;
    Ok(())
}

/// `Some(iso)` = went cold; `None` = warmed up. Same double-write guard as
/// `set_dormant`: the first cold stamp wins until it actually thaws.
pub fn set_relationship_cold(conn: &Connection, since: Option<&str>) -> rusqlite::Result<usize> {
    match since {
        Some(iso) => conn.execute(
            "UPDATE relationship SET cold_since = ?1 WHERE id = 1 AND cold_since IS NULL",
            params![iso],
        ),
        None => conn.execute(
            "UPDATE relationship SET cold_since = NULL WHERE id = 1 AND cold_since IS NOT NULL",
            [],
        ),
    }
}

pub fn set_relationship_tick_date(conn: &Connection, date: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE relationship SET last_tick_date = ?1 WHERE id = 1",
        params![date],
    )?;
    Ok(())
}

/// Recompute-then-store for one local day. Idempotent by construction: the
/// caller derives `ev` from source tables every time, so a second call with
/// unchanged sources writes the same numbers.
pub fn upsert_progress(
    conn: &Connection,
    date: &str,
    ev: &DayEvidence,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO relationship_progress
             (date, present, hits, misses, disagreements, shut_ups, score, computed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(date) DO UPDATE SET
             present = excluded.present,
             hits = excluded.hits,
             misses = excluded.misses,
             disagreements = excluded.disagreements,
             shut_ups = excluded.shut_ups,
             score = excluded.score,
             computed_at = excluded.computed_at",
        params![
            date,
            ev.present,
            ev.hits,
            ev.misses,
            ev.disagreements,
            ev.shut_ups,
            ev.score,
            clock::now_iso()
        ],
    )?;
    Ok(())
}

pub fn progress_on_day(conn: &Connection, date: &str) -> rusqlite::Result<Option<DayEvidence>> {
    let mut stmt = conn.prepare(
        "SELECT present, hits, misses, disagreements, shut_ups, score
         FROM relationship_progress WHERE date = ?1",
    )?;
    let mut rows = stmt.query_map(params![date], |row| {
        Ok(DayEvidence {
            present: row.get(0)?,
            hits: row.get(1)?,
            misses: row.get(2)?,
            disagreements: row.get(3)?,
            shut_ups: row.get(4)?,
            score: row.get(5)?,
        })
    })?;
    rows.next().transpose()
}

/// Cumulative evidence BEFORE `date` — today never counts, so a single hit
/// cannot promote the relationship mid-day (plan §3.3).
pub fn progress_sum_before(conn: &Connection, date: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(score), 0) FROM relationship_progress WHERE date < ?1",
        params![date],
        |r| r.get(0),
    )
}

/// Cumulative evidence in the half-open window `[from, before)` — the rolling
/// 30-day view used by the promotion rule (plan §3.3).
pub fn progress_sum_between(
    conn: &Connection,
    from: &str,
    before: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(score), 0) FROM relationship_progress
         WHERE date >= ?1 AND date < ?2",
        params![from, before],
        |r| r.get(0),
    )
}

pub fn add_relationship_log(conn: &Connection, stage: i64, note: &str) -> rusqlite::Result<i64> {
    add_relationship_log_kind(conn, "stage", stage, note)
}

pub fn add_relationship_log_kind(
    conn: &Connection,
    kind: &str,
    stage: i64,
    note: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO relationship_log (kind, stage, unlocked_at, note) VALUES (?1, ?2, ?3, ?4)",
        params![kind, stage, clock::now_iso(), note],
    )?;
    Ok(conn.last_insert_rowid())
}

/// (id, kind, stage, note) — oldest first, un-announced only.
pub fn pending_relationship_logs(
    conn: &Connection,
) -> rusqlite::Result<Vec<(i64, String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, stage, note FROM relationship_log
         WHERE notified_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.collect()
}

pub fn mark_relationship_notified(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE relationship_log SET notified_at = ?1 WHERE id = ?2 AND notified_at IS NULL",
        params![clock::now_iso(), id],
    )?;
    Ok(n > 0)
}

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
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tool_calls
           (ts, tool, action, outcome, result_status, params_summary, prompt_trace, remote, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            clock::now_iso(),
            tool,
            action,
            outcome,
            result_status,
            params_summary,
            prompt_trace,
            remote as i64,
            duration_ms
        ],
    )?;
    Ok(())
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
            tool_label, filter_report, duration_ms, user_text, reply_text, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
    fn v7_database_upgrades_to_v8_idempotently() {
        // v7-era shape: tool_calls exists WITHOUT the agent audit columns and
        // mcp_servers does not exist yet (spec-s2 §1).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 kind TEXT NOT NULL, content TEXT NOT NULL, role TEXT, session_id TEXT,
                 consumed INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_event_id INTEGER,
                 confidence REAL NOT NULL DEFAULT 0.6,
                 importance INTEGER NOT NULL DEFAULT 3, invalid_at TEXT);
             CREATE TABLE preferences (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 category TEXT NOT NULL, content TEXT NOT NULL,
                 confidence REAL NOT NULL DEFAULT 0.5, invalid_at TEXT);
             CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 content TEXT NOT NULL, source_fact_id INTEGER);
             CREATE TABLE tool_calls (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                 tool TEXT NOT NULL, action TEXT NOT NULL, outcome TEXT NOT NULL,
                 detail TEXT, duration_ms INTEGER);
             PRAGMA user_version = 7;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES ('2026-11-10T00:00:00Z', 'chat', 'v7 row')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_calls (ts, tool, action, outcome)
             VALUES ('2026-11-10T00:00:00Z', 'calendar', 'agenda', 'ok')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);

        // New table appeared, legacy audit row survived with NULL defaults.
        let servers: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(servers, 0);
        let records = tool_call_records(&conn, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].action, "agenda");
        assert!(records[0].params_summary.is_none());
        assert!(!records[0].remote);
        assert_eq!(all_events(&conn).len(), 1);

        // Second run is a no-op — no duplicate columns, no data loss.
        migrate(&conn).unwrap();
        assert_eq!(all_events(&conn).len(), 1);
        assert_eq!(tool_call_records(&conn, 10).unwrap().len(), 1);
    }

    #[test]
    fn tool_call_records_newest_first_with_limit() {
        let conn = mem();
        for n in 1..=3 {
            log_agent_tool_call(
                &conn,
                "mcp",
                &format!("tool{n}"),
                "ok",
                "auto_allowed",
                None,
                None,
                false,
                Some(n),
            )
            .unwrap();
        }
        let all = tool_call_records(&conn, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].action, "tool3", "newest first");
        assert_eq!(all[2].action, "tool1");

        let capped = tool_call_records(&conn, 2).unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].action, "tool3");
    }

    #[test]
    fn dialogue_bubbles_flat_ordered_and_session_scoped() {
        let conn = mem();
        add_event_meta(&conn, "dialogue", "早上好", Some("user"), Some("default")).unwrap();
        add_event_meta(&conn, "dialogue", "早。今天有雨。", Some("assistant"), Some("default")).unwrap();
        add_event_meta(&conn, "dialogue", "明天呢？", Some("user"), Some("default")).unwrap();
        // noise: non-dialogue + other session — must be excluded.
        add_event_meta(&conn, "perception", "窗口打开", None, None).unwrap();
        add_event_meta(&conn, "dialogue", "别的会话", Some("user"), Some("other")).unwrap();

        let rows = dialogue_bubbles(&conn, "default", 50, None).unwrap();
        assert_eq!(rows.len(), 3, "flat rows: proactive/reply-less kept, noise dropped");
        assert_eq!(rows[0].role, "user");
        assert_eq!(rows[0].content, "早上好");
        assert_eq!(rows[2].role, "user");
        assert!(rows[1].ts.contains("20"), "ts carried from events");

        let capped = dialogue_bubbles(&conn, "default", 2, None).unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].content, "早。今天有雨。", "limit takes the newest, oldest first out");

        // Pagination cursor: capped = [id2, id3]; page before id2 → only id1.
        let page2 = dialogue_bubbles(&conn, "default", 2, Some(capped[0].id)).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].content, "早上好", "cursor returns strictly older rows");
        let page1_only = dialogue_bubbles(&conn, "default", 1, Some(rows[2].id)).unwrap();
        assert_eq!(page1_only.len(), 1);
        assert_eq!(page1_only[0].content, "早。今天有雨。", "cursor excludes itself and everything after");
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
    fn relationship_stage_is_monotonic_and_cold_flips_once() {
        let conn = mem();
        assert!(relationship_state(&conn).unwrap().is_none());
        let s = ensure_relationship(&conn).unwrap();
        assert_eq!(s.stage, 0);
        // Second ensure is a no-op (single row, no duplicate insert).
        ensure_relationship(&conn).unwrap();

        set_relationship_stage(&conn, 1).unwrap();
        let s = relationship_state(&conn).unwrap().unwrap();
        assert_eq!(s.stage, 1);
        assert!(s.stage_since.is_some(), "promotion stamps stage_since");

        // Going backwards is a silent no-op — invariant 1 (never rewinds).
        set_relationship_stage(&conn, 0).unwrap();
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 1);
        set_relationship_stage(&conn, 2).unwrap();
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 2);

        // Cold stamps once, thaws once (double writes are no-ops).
        assert_eq!(set_relationship_cold(&conn, Some("2026-11-19T00:00:00Z")).unwrap(), 1);
        assert_eq!(set_relationship_cold(&conn, Some("2026-11-20T00:00:00Z")).unwrap(), 0);
        assert_eq!(set_relationship_cold(&conn, None).unwrap(), 1);
        assert_eq!(set_relationship_cold(&conn, None).unwrap(), 0);
        assert_eq!(relationship_state(&conn).unwrap().unwrap().stage, 2, "cold never rewinds stage");
    }

    #[test]
    fn progress_upsert_is_recompute_not_accumulate() {
        let conn = mem();
        let ev = DayEvidence {
            present: 1,
            hits: 1,
            misses: 0,
            disagreements: 1,
            shut_ups: 0,
            score: 3,
        };
        upsert_progress(&conn, "2026-11-19", &ev).unwrap();
        // Same source data recomputed → the row is replaced, not summed.
        upsert_progress(&conn, "2026-11-19", &ev).unwrap();
        let got = progress_on_day(&conn, "2026-11-19").unwrap().unwrap();
        assert_eq!(got, ev, "double recompute must not double the score");

        // A changed source (one more miss) overwrites the day honestly.
        let ev2 = DayEvidence { misses: 1, score: 2, ..ev.clone() };
        upsert_progress(&conn, "2026-11-19", &ev2).unwrap();
        assert_eq!(progress_on_day(&conn, "2026-11-19").unwrap().unwrap().score, 2);
    }

    #[test]
    fn progress_sum_before_excludes_today() {
        let conn = mem();
        for (d, score) in [("2026-11-17", 2), ("2026-11-18", 1), ("2026-11-19", 3)] {
            upsert_progress(&conn, d, &DayEvidence { score, ..Default::default() }).unwrap();
        }
        // Today (11-19) never counts toward promotion — no mid-day jump.
        assert_eq!(progress_sum_before(&conn, "2026-11-19").unwrap(), 3);
        assert_eq!(progress_sum_before(&conn, "2026-11-20").unwrap(), 6);
        assert_eq!(progress_sum_before(&conn, "2026-11-17").unwrap(), 0);
    }

    #[test]
    fn relationship_reset_keeps_memory_and_growth() {
        let conn = mem();
        ensure_growth(&conn).unwrap();
        set_growth_stage(&conn, 2).unwrap();
        add_event(&conn, "dialogue", "hi").unwrap();
        ensure_relationship(&conn).unwrap();
        set_relationship_stage(&conn, 2).unwrap();
        add_relationship_log(&conn, 2, "会顶嘴了。").unwrap();
        upsert_progress(&conn, "2026-11-19", &DayEvidence { score: 2, ..Default::default() }).unwrap();

        forget_relationship(&conn).unwrap();
        assert!(relationship_state(&conn).unwrap().is_none());
        assert!(pending_relationship_logs(&conn).unwrap().is_empty());
        assert!(progress_on_day(&conn, "2026-11-19").unwrap().is_none());
        // Memory and the plant are untouched — "start over socially" only.
        assert_eq!(growth_state(&conn).unwrap().unwrap().stage, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        // forget_all still cascades the relationship tables.
        ensure_relationship(&conn).unwrap();
        add_relationship_log(&conn, 1, "熟络了。").unwrap();
        forget_all(&conn).unwrap();
        assert!(relationship_state(&conn).unwrap().is_none());
        assert!(pending_relationship_logs(&conn).unwrap().is_empty());
    }

    #[test]
    fn settings_upsert_remove_and_onboarding_cascade() {
        let conn = mem();
        // Missing key reads as None — that's the "old user" signal.
        assert!(get_setting(&conn, "onboarding_done").unwrap().is_none());
        set_setting(&conn, "onboarding_done", "1").unwrap();
        // Upsert overwrites, never duplicates.
        set_setting(&conn, "onboarding_done", "0").unwrap();
        assert_eq!(get_setting(&conn, "onboarding_done").unwrap().as_deref(), Some("0"));
        assert_eq!(remove_setting(&conn, "onboarding_done").unwrap(), 1);
        assert!(get_setting(&conn, "onboarding_done").unwrap().is_none());

        // Skipped answers are recorded too (skipped=1 is a signal).
        record_onboarding_answer(&conn, "q1", "我妈", false).unwrap();
        record_onboarding_answer(&conn, "q4", "", true).unwrap();
        let answers = onboarding_answers(&conn).unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0], ("q1".into(), "我妈".into(), false));
        assert_eq!(answers[1], ("q4".into(), "".into(), true));

        // Nuclear forget clears the onboarding state: the honest reading of
        // "forget everything" includes "re-introduce yourself".
        set_setting(&conn, "age_group", "adult").unwrap();
        forget_all(&conn).unwrap();
        assert!(get_setting(&conn, "age_group").unwrap().is_none());
        assert!(onboarding_answers(&conn).unwrap().is_empty());
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

    fn cal_row(external_id: &str, start: &str, title: &str) -> CalendarRow {
        CalendarRow {
            external_id: external_id.into(),
            start_at: start.into(),
            end_at: start.into(),
            title: title.into(),
            calendar_name: Some("工作".into()),
            all_day: false,
            attendee_count: 3,
            declined: false,
            canceled: false,
            availability: Some("busy".into()),
            reminded_at: None,
            fetched_at: String::new(),
        }
    }

    #[test]
    fn calendar_replace_preserves_reminded_at_and_sweeps_cancelled() {
        let mut conn = mem();
        let t0 = "2026-11-19T06:00:00+00:00";
        calendar_replace(&mut conn, &[cal_row("a", t0, "评审"), cal_row("b", t0, "一对一")], "2026-11-19T05:00:00+00:00").unwrap();
        assert_eq!(calendar_between(&conn, "2026-11-19T00:00:00+00:00", "2026-11-20T00:00:00+00:00").unwrap().len(), 2);

        // Reminder fired, then a refresh arrives: the flag must survive,
        // otherwise the same meeting is announced again 5 minutes later.
        assert!(mark_calendar_reminded(&conn, "a", t0).unwrap());
        assert!(!mark_calendar_reminded(&conn, "a", t0).unwrap()); // idempotent
        // "b" was cancelled upstream: absent from the next fetch.
        calendar_replace(&mut conn, &[cal_row("a", t0, "评审")], "2026-11-19T05:05:00+00:00").unwrap();
        let rows = calendar_between(&conn, "2026-11-19T00:00:00+00:00", "2026-11-20T00:00:00+00:00").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].external_id, "a");
        assert!(rows[0].reminded_at.is_some());
        assert_eq!(calendar_last_fetch(&conn).unwrap().as_deref(), Some("2026-11-19T05:05:00+00:00"));
    }

    #[test]
    fn calendar_next_after_picks_the_earliest_upcoming() {
        let mut conn = mem();
        calendar_replace(
            &mut conn,
            &[
                cal_row("late", "2026-11-19T09:00:00+00:00", "下午"),
                cal_row("early", "2026-11-19T06:00:00+00:00", "上午"),
            ],
            "2026-11-19T05:00:00+00:00",
        )
        .unwrap();
        let next = calendar_next_after(&conn, "2026-11-19T05:00:00+00:00").unwrap().unwrap();
        assert_eq!(next.external_id, "early");
        assert!(calendar_next_after(&conn, "2026-11-19T10:00:00+00:00").unwrap().is_none());
    }

    #[test]
    fn tool_audit_counts_and_cascades_on_forget() {
        let conn = mem();
        log_tool_call(&conn, "calendar", "agenda", "ok", Some("2 events, 40ms"), Some(40)).unwrap();
        log_tool_call(&conn, "calendar", "next", "denied", None, None).unwrap();
        let (ok, total) = tool_calls_summary(
            &conn,
            "calendar",
            "1970-01-01T00:00:00Z",
            "2999-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!((ok, total), (1, 2));
        assert_eq!(tool_calls_recent(&conn, 5).unwrap().len(), 2);
        forget_all(&conn).unwrap();
        assert!(tool_calls_recent(&conn, 5).unwrap().is_empty());
        assert!(calendar_last_fetch(&conn).unwrap().is_none());
    }

    #[test]
    fn v9_creates_trace_tables() {
        let _conn = mem();
        // Fresh schema carries the v9 tables; migrate on a legacy v1 db too.
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                     kind TEXT NOT NULL, content TEXT NOT NULL);
                 CREATE TABLE facts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL,
                     content TEXT NOT NULL, source_event_id INTEGER, invalid_at TEXT);
                 CREATE TABLE boundaries (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     content TEXT NOT NULL)",
            )
            .unwrap();
        migrate(&legacy).unwrap();
        assert_eq!(user_version(&legacy).unwrap(), SCHEMA_VERSION);
        let n: i64 = legacy
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
                 AND name IN ('turn_trace','trace_steps')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "migrated db must carry both trace tables");
    }

    #[test]
    fn trace_turn_persist_get_and_find_by_event_roundtrip() {
        let conn = mem();
        let draft = TraceTurnDraft {
            session_id: "s1".into(),
            kind: "model".into(),
            model_used: true,
            tool_label: Some("builtin:memory_search".into()),
            filter_report: Some(r#"{"explicit":false,"hedged":false}"#.into()),
            duration_ms: Some(1234),
            user_text: Some("帮我看看明天安排".into()),
            reply_text: Some("明天上午有两条".into()),
            steps: vec![
                TraceStepDraft {
                    step_kind: "crisis_check".into(),
                    title: "危机拦截检查".into(),
                    prompt: None,
                    output: None,
                    args_json: None,
                    summary: None,
                    outcome: Some("miss".into()),
                    duration_ms: None,
                    user_text: None,
                },
                TraceStepDraft {
                    step_kind: "llm_round".into(),
                    title: "直接对话".into(),
                    prompt: Some("SYS".into()),
                    output: Some("REPLY".into()),
                    args_json: None,
                    summary: None,
                    outcome: Some("answered".into()),
                    duration_ms: Some(900),
                    user_text: Some("在吗".into()),
                },
            ],
        };
        let uid = add_event(&conn, "dialogue", "user text").unwrap();
        let aid = add_event(&conn, "dialogue", "assistant reply").unwrap();
        let turn_id = trace_turn_persist(&conn, &draft, Some(uid), Some(aid)).unwrap();

        // Full readback: master + ordered steps.
        let trace = turn_trace_get(&conn, turn_id).unwrap().expect("turn exists");
        assert_eq!(trace.kind, "model");
        assert!(trace.model_used);
        assert_eq!(trace.user_event_id, Some(uid));
        assert_eq!(trace.assistant_event_id, Some(aid));
        assert_eq!(trace.steps.len(), 2);
        assert_eq!(trace.steps[0].seq, 1);
        assert_eq!(trace.steps[0].step_kind, "crisis_check");
        assert_eq!(trace.steps[1].seq, 2);
        assert_eq!(trace.steps[1].prompt.as_deref(), Some("SYS"));

        // Both sides of the dialogue resolve to the same turn; a stranger id
        // resolves to nothing.
        assert_eq!(turn_trace_find_by_event(&conn, uid).unwrap(), Some(turn_id));
        assert_eq!(turn_trace_find_by_event(&conn, aid).unwrap(), Some(turn_id));
        assert_eq!(turn_trace_find_by_event(&conn, 999_999).unwrap(), None);
        assert!(turn_trace_get(&conn, 999_999).unwrap().is_none());
    }
}
