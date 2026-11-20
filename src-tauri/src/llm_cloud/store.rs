//! 条目存储：settings 表 ↔ 进程级缓存。
//!
//! 缓存的意义：模型腿取 client 时不能去抢 DB 锁（对话线程持有连接时
//! 切换模型会死锁），所以启动时回填一次，之后每次写操作刷新。

use std::sync::{OnceLock, RwLock};

use rusqlite::Connection;

use super::CloudEntry;
use crate::memory::db::{get_setting, set_setting};

const KEY_ENTRIES: &str = "llm_cloud_entries";

static CACHE: OnceLock<RwLock<Vec<CloudEntry>>> = OnceLock::new();

fn cache() -> &'static RwLock<Vec<CloudEntry>> {
    CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

/// 进程内快照（模型腿 / 视图用）。
pub fn entries() -> Vec<CloudEntry> {
    cache().read().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(test)]
pub fn set_cache_for_test(list: Vec<CloudEntry>) {
    if let Ok(mut w) = cache().write() {
        *w = list;
    }
}

fn set_cache(list: Vec<CloudEntry>) {
    if let Ok(mut w) = cache().write() {
        *w = list;
    }
}

/// settings → 条目列表。损坏的 JSON 视为空（log warn），绝不 panic。
pub fn load(conn: &Connection) -> Vec<CloudEntry> {
    let Some(raw) = get_setting(conn, KEY_ENTRIES).ok().flatten() else {
        return Vec::new();
    };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str(&raw) {
        Ok(list) => list,
        Err(e) => {
            log::warn!("llm_cloud_entries is not valid JSON ({e}) — treating as empty");
            Vec::new()
        }
    }
}

pub(super) fn save_all(conn: &Connection, list: &[CloudEntry]) -> Result<(), String> {
    let json = serde_json::to_string(list).map_err(|e| e.to_string())?;
    set_setting(conn, KEY_ENTRIES, &json).map_err(|e| e.to_string())?;
    set_cache(list.to_vec());
    Ok(())
}

/// 启动回填（lib.rs setup 调用，早于 sidecar 启动）。
pub fn refresh_cache(conn: &Connection) {
    set_cache(load(conn));
}

pub fn find(entry_id: &str) -> Option<CloudEntry> {
    entries().into_iter().find(|e| e.id == entry_id)
}
