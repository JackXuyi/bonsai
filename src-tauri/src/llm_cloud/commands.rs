//! 命令层（spec-m2 §2）：读写清单、删条目、一枪测试。
//!
//! 只有命令层能碰 DB 连接；纯逻辑在 `super`，存储与缓存在 `super::store`。

use tauri::Emitter;

use super::{build_entry, humanize, load, probe_entry, save_entry, remove_entry, CloudEntryInput, CloudEntryView, TestReport};
use crate::memory::MemoryStore;

#[tauri::command]
pub fn llm_cloud_entries_get(
    store: tauri::State<'_, MemoryStore>,
) -> Result<Vec<CloudEntryView>, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    Ok(load(&conn).iter().map(super::CloudEntry::view).collect())
}

#[tauri::command]
pub fn llm_cloud_entry_save(
    app: tauri::AppHandle,
    store: tauri::State<'_, MemoryStore>,
    entry: CloudEntryInput,
) -> Result<CloudEntryView, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    let view = save_entry(&conn, entry).map_err(humanize)?;
    drop(conn);
    // 页头状态可能因编辑改名而变化（list 是唯一真源，通知前端重读）。
    let _ = app.emit("llm:cloud:changed", ());
    Ok(view)
}

#[tauri::command]
pub fn llm_cloud_entry_remove(
    app: tauri::AppHandle,
    store: tauri::State<'_, MemoryStore>,
    id: String,
) -> Result<(), String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    remove_entry(&conn, &id)?;
    drop(conn);
    let _ = app.emit("llm:cloud:changed", ());
    Ok(())
}

#[tauri::command]
pub fn llm_cloud_test(
    store: tauri::State<'_, MemoryStore>,
    entry: CloudEntryInput,
) -> Result<TestReport, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    let built = build_entry(&entry, &load(&conn)).map_err(humanize)?;
    let started = std::time::Instant::now();
    probe_entry(&built)?;
    Ok(TestReport { latency_ms: started.elapsed().as_millis() as u64 })
}
