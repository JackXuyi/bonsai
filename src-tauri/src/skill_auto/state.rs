//! KV state for skill auto-extract.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::memory::db;

use super::FAIL_BLACKLIST;

pub const KEY_STATE: &str = "skill_auto_state";
pub const KEY_META: &str = "skill_auto_meta";
pub const KEY_ENABLED: &str = "skill_auto_enabled";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoState {
    #[serde(default)]
    pub pending: Vec<String>,
    #[serde(default)]
    pub consumed: Vec<String>,
    #[serde(default)]
    pub failures: HashMap<String, u32>,
    #[serde(default)]
    pub last_run_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoMetaEntry {
    pub source_task_ids: Vec<String>,
    pub created_at: String,
    pub model: String,
    pub confidence: f64,
    /// M3：启用后被注入对话的次数。
    #[serde(default)]
    pub use_count: u64,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

pub type AutoMeta = HashMap<String, AutoMetaEntry>;

pub fn load_state(conn: &Connection) -> AutoState {
    db::get_setting(conn, KEY_STATE)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(conn: &Connection, state: &AutoState) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(state).unwrap_or_else(|_| "{}".into());
    db::set_setting(conn, KEY_STATE, &raw)
}

pub fn load_meta(conn: &Connection) -> AutoMeta {
    db::get_setting(conn, KEY_META)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_meta(conn: &Connection, meta: &AutoMeta) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(meta).unwrap_or_else(|_| "{}".into());
    db::set_setting(conn, KEY_META, &raw)
}

pub fn is_feature_enabled(conn: &Connection) -> bool {
    matches!(
        db::get_setting(conn, KEY_ENABLED).ok().flatten().as_deref(),
        Some("true") | Some("1")
    )
}

pub fn set_feature_enabled(conn: &Connection, on: bool) -> rusqlite::Result<()> {
    db::set_setting(conn, KEY_ENABLED, if on { "true" } else { "false" })
}

/// T1：任务收尾后轻量入队（热路径不调模型）。
pub fn enqueue_candidate(conn: &Connection, task_id: &str) -> rusqlite::Result<()> {
    if task_id.trim().is_empty() {
        return Ok(());
    }
    let mut st = load_state(conn);
    if st.consumed.iter().any(|x| x == task_id) || st.pending.iter().any(|x| x == task_id) {
        return Ok(());
    }
    if st.failures.get(task_id).copied().unwrap_or(0) >= FAIL_BLACKLIST {
        return Ok(());
    }
    st.pending.push(task_id.to_string());
    save_state(conn, &st)
}
