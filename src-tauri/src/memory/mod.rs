//! L2 — Memory layer: local SQLite, local-first, explainable, forgettable.
//!
//! Research constraints (report §4.2):
//! - Must be explainable: every derived fact traces back to source events.
//! - Must be forgettable: user says "forget this" → truly delete (compliance
//!   Art. 16 requires deletability). Superseded facts are stamped `invalid_at`.
//! - Must be local-first: relationship data never goes to the cloud by default.
//!
//! Retrieval plan: hybrid (vector + keyword + time decay). This scaffold ships
//! the keyword leg (LIKE) only; the vector index is a follow-up milestone.

pub mod consolidate;
pub mod db;

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
) -> Result<i64, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::add_event(&conn, &kind, &content).map_err(|e| e.to_string())
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

/// Hybrid retrieval, keyword leg. Active facts first (not invalidated),
/// then raw events. Vector + time-decay legs come later.
#[tauri::command]
pub fn search(
    state: tauri::State<'_, MemoryStore>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<MemoryHit>, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    db::search(&conn, &query, limit.unwrap_or(20).max(1) as usize).map_err(|e| e.to_string())
}

/// Nightly consolidation job (report §4.3): compress → extract → resolve
/// contradictions → delete. Exposed as a command for the scaffold so the job
/// can be triggered manually; the scheduler is a follow-up milestone.
#[tauri::command]
pub fn consolidate(state: tauri::State<'_, MemoryStore>) -> Result<consolidate::Report, String> {
    let conn = state.inner().0.lock().map_err(|e| e.to_string())?;
    consolidate::run(&conn).map_err(|e| e.to_string())
}
