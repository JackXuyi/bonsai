//! L2 — Memory layer: local SQLite, local-first, explainable, forgettable.
//!
//! Research constraints (report §4.2) and plan (docs/zh/plan-memory.md):
//! - Explainable: every derived fact traces back to source events.
//! - Forgettable: "forget this" truly deletes (compliance Art. 16).
//! - Local-first: relationship data never leaves the machine by default.
//! - Hybrid retrieval: keyword + time decay (vector leg is a follow-up).

pub mod clock;
pub mod consolidate;
pub mod db;
pub mod eval;
pub mod extract;
pub mod overview;
pub mod scheduler;

use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;

pub struct MemoryStore(pub Mutex<Connection>);

impl MemoryStore {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        Ok(MemoryStore(Mutex::new(db::open(path)?)))
    }
}

#[derive(Debug, Serialize)]
pub struct MemoryHit {
    pub kind: String,
    pub id: i64,
    pub ts: String,
    pub content: String,
    pub invalidated: bool,
}

#[tauri::command]
pub fn add_event(
    state: tauri::State<'_, MemoryStore>,
    kind: String,
    content: String,
    role: Option<String>,
    session_id: Option<String>,
) -> Result<i64, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::add_event_meta(&conn, &kind, &content, role.as_deref(), session_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_fact(
    state: tauri::State<'_, MemoryStore>,
    content: String,
    source_event_id: Option<i64>,
) -> Result<i64, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::add_fact(&conn, &content, source_event_id).map_err(|e| e.to_string())
}

/// Hybrid retrieval (keyword + decay). `withDecayed: false` = 翻旧账 mode.
#[tauri::command]
pub fn search(
    state: tauri::State<'_, MemoryStore>,
    query: String,
    limit: Option<u32>,
    with_decayed: Option<bool>,
) -> Result<Vec<MemoryHit>, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::search(&conn, &query, limit.unwrap_or(20).max(1) as usize, with_decayed.unwrap_or(true))
        .map_err(|e| e.to_string())
}

/// Manual consolidation (frontend button / debugging). Uses the local LLM
/// when reachable, rules otherwise (plan §5.2). The nightly schedule lives
/// in `scheduler::spawn`.
#[tauri::command]
pub fn consolidate(state: tauri::State<'_, MemoryStore>) -> Result<consolidate::Report, String> {
    let window_end = clock::now_iso();
    // Store-backed entry: extraction runs with zero locks held.
    consolidate::run_with_local_or_rules(
        state.inner(),
        &consolidate::Options {
            trigger: "manual",
            window_start: "1970-01-01T00:00:00Z",
            window_end: &window_end,
        },
    )
    .map_err(|e| e.to_string())
}

/// Compliance Art. 16: forget a whole conversation by session id.
#[tauri::command]
pub fn forget_session(state: tauri::State<'_, MemoryStore>, session_id: String) -> Result<usize, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::forget_session(&conn, &session_id).map_err(|e| e.to_string())
}

/// Compliance Art. 16: forget everything.
#[tauri::command]
pub fn forget_all(state: tauri::State<'_, MemoryStore>) -> Result<(), String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::forget_all(&conn).map_err(|e| e.to_string())
}

/// 记忆页总览（spec-m3）：只读快照，`limit` 缺省 20、上限 100。
#[tauri::command]
pub fn memory_overview(
    state: tauri::State<'_, MemoryStore>,
    limit: Option<u32>,
) -> Result<overview::MemoryOverview, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    overview::build(&conn, limit).map_err(|e| e.to_string())
}
