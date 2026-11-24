//! 用户画像（docs/zh/plan-user-profile.md §2–§4）：身份快照，不是事实。
//!
//! 与 facts 的分工（方案 §1 的核心判断）：facts 是事件性留痕（「用户在
//! 某日说在用 Cursor」），画像是身份 KV（「用户现在用 Cursor」）——可覆盖、
//! 每轮固定注入。一份抽取两个出口：抽取层照旧落 facts，画像走本模块。
//!
//! 三条硬约束：
//! 1. **key 白名单**（方案 §2）：收入/健康/感情/住址等敏感字段永远进不来——
//!    白名单外的 key 在写入层静默丢弃（宁漏勿误），不是前端过滤的事。
//! 2. **manual 保护**（方案 §3.3）：用户在工具页亲口写的值，任何自动抽取
//!    （rule/model）不得覆盖。
//! 3. **覆盖留痕**：被替换的旧值进 `user_profile_history`，不静默消失；
//!    「忘掉一切」级联清空两张表（compliance Art. 16）。

use rusqlite::{params, Connection};
use serde::Serialize;

use super::clock;

/// 画像字段白名单（方案 §2）。顺序即展示/注入顺序——name 在前，身份先于
/// 环境。新增字段 = 加一行 + 前端 label 映射补一格，都要过一次产品裁决。
pub const KEYS: &[&str] = &["name", "occupation", "tools", "city", "projects", "schedule"];

/// key → 中文标签（方案 §6.2 / prompt 注入共用一处，不许两份映射）。
pub fn label(key: &str) -> &'static str {
    match key {
        "name" => "称呼",
        "occupation" => "职业",
        "tools" => "常用工具",
        "city" => "城市",
        "projects" => "在做的事",
        "schedule" => "作息",
        _ => "",
    }
}

pub fn is_valid_key(key: &str) -> bool {
    KEYS.contains(&key)
}

/// 单值长度上限（方案 §5：整段 ≤200 字符的预算按 6 字段分摊）。
const VALUE_MAX_CHARS: usize = 100;

/// 来源：rule/model = 自动抽取，manual = 工具页人手写的（最高优先级）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRow {
    pub key: String,
    pub value: String,
    pub source: String,
    pub confidence: f64,
    pub updated_at: String,
}

/// 写入一条画像（方案 §4 冲突仲裁）。返回是否真正落库。
///
/// - 白名单外 key / 空值 → false（静默丢弃，调用方不报错——宁漏勿误）；
/// - 已有 manual 值且本次非 manual → false（manual 保护）；
/// - 值与现状完全一致 → false（updated_at 不该因重读而抖动）；
/// - 其余 → 旧值进 history，新值 UPSERT。
pub fn apply(
    conn: &Connection,
    key: &str,
    value: &str,
    source: &str,
    source_event_id: Option<i64>,
    confidence: f64,
) -> rusqlite::Result<bool> {
    if !is_valid_key(key) {
        return Ok(false);
    }
    let value = value.trim();
    if value.is_empty() {
        return Ok(false);
    }
    // 100 字符截断：画像值是一行短句，不是一段简介。
    let value: String = value.chars().take(VALUE_MAX_CHARS).collect();

    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT value, source FROM user_profile WHERE key = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    if let Some((old_value, old_source)) = existing {
        if old_source == "manual" && source != "manual" {
            return Ok(false); // 用户亲口定的身份 > 模型猜的
        }
        if old_value == value {
            return Ok(false);
        }
        archive(conn, key, &old_value, &old_source)?;
        conn.execute(
            "UPDATE user_profile SET value = ?1, source = ?2, source_event_id = ?3,
                 confidence = ?4, updated_at = ?5 WHERE key = ?6",
            params![value, source, source_event_id, confidence, clock::now_iso(), key],
        )?;
    } else {
        conn.execute(
            "INSERT INTO user_profile (key, value, source, source_event_id, confidence, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![key, value, source, source_event_id, confidence, clock::now_iso()],
        )?;
    }
    Ok(true)
}

/// 旧值归档（覆盖与显式删除共用）。历史只进不出，供审计与「我改错了」回看。
fn archive(conn: &Connection, key: &str, value: &str, source: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO user_profile_history (key, value, source, valid_until)
         VALUES (?1, ?2, ?3, ?4)",
        params![key, value, source, clock::now_iso()],
    )?;
    Ok(())
}

/// 全量画像，按白名单顺序（稳定：prompt 注入顺序确定，前端展示顺序也确定）。
pub fn get_all(conn: &Connection) -> rusqlite::Result<Vec<ProfileRow>> {
    let mut stmt = conn.prepare(
        "SELECT key, value, source, confidence, updated_at FROM user_profile",
    )?;
    let mut rows: Vec<ProfileRow> = stmt
        .query_map([], |row| {
            Ok(ProfileRow {
                key: row.get(0)?,
                value: row.get(1)?,
                source: row.get(2)?,
                confidence: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.sort_by_key(|r| KEYS.iter().position(|k| *k == r.key).unwrap_or(usize::MAX));
    Ok(rows)
}

/// 工具页「清」：整条删除进 history（方案 §6.2）。未知 key → 0 行，无害。
pub fn delete(conn: &Connection, key: &str) -> rusqlite::Result<usize> {
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT value, source FROM user_profile WHERE key = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((value, source)) = existing {
        archive(conn, key, &value, &source)?;
    }
    conn.execute("DELETE FROM user_profile WHERE key = ?1", params![key])
}

// ---------------------------------------------------------------- commands
// （工具页画像卡的数据链路，方案 §6.3。UI 保存一律 source='manual'。）

#[tauri::command]
pub fn profile_list(state: tauri::State<'_, super::MemoryStore>) -> Result<Vec<ProfileRow>, String> {
    state
        .inner()
        .with_read(|conn| get_all(conn))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn profile_set(
    state: tauri::State<'_, super::MemoryStore>,
    key: String,
    value: String,
) -> Result<bool, String> {
    let conn = state.inner().write_guard().map_err(|e| e.to_string())?;
    apply(&conn, &key, &value, "manual", None, 1.0).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn profile_delete(state: tauri::State<'_, super::MemoryStore>, key: String) -> Result<usize, String> {
    let conn = state.inner().write_guard().map_err(|e| e.to_string())?;
    delete(&conn, &key).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    #[test]
    fn apply_inserts_and_reads_back_in_whitelist_order() {
        let conn = mem();
        let ev1 = db::add_event(&conn, "chat", "我叫许一").unwrap();
        let ev2 = db::add_event(&conn, "chat", "我是做 QA 的").unwrap();
        assert!(apply(&conn, "occupation", "QA / 质量管理", "rule", Some(ev2), 0.7).unwrap());
        assert!(apply(&conn, "name", "许一", "model", Some(ev1), 0.8).unwrap());
        let rows = get_all(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "name", "白名单顺序，不是写入顺序");
        assert_eq!(rows[0].value, "许一");
        assert_eq!(rows[1].key, "occupation");
    }

    #[test]
    fn whitelist_rejects_unknown_keys_and_empty_values() {
        let conn = mem();
        // 敏感字段永远进不来：白名单是硬闸，不是前端过滤。
        assert!(!apply(&conn, "income", "年薪 50w", "model", None, 0.9).unwrap());
        assert!(!apply(&conn, "health", "有高血压", "model", None, 0.9).unwrap());
        assert!(!apply(&conn, "name", "   ", "rule", None, 0.7).unwrap());
        let rows = get_all(&conn).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn apply_truncates_overlong_values() {
        let conn = mem();
        let long = "长".repeat(300);
        assert!(apply(&conn, "projects", &long, "rule", None, 0.7).unwrap());
        let rows = get_all(&conn).unwrap();
        assert_eq!(rows[0].value.chars().count(), VALUE_MAX_CHARS);
    }

    #[test]
    fn overwrite_moves_old_value_to_history() {
        let conn = mem();
        apply(&conn, "occupation", "QA", "rule", None, 0.7).unwrap();
        assert!(apply(&conn, "occupation", "产品经理", "model", None, 0.8).unwrap());
        let rows = get_all(&conn).unwrap();
        assert_eq!(rows[0].value, "产品经理");
        assert_eq!(rows[0].source, "model");
        let hist: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile_history WHERE key = 'occupation'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hist, 1, "被替换的旧值进 history，不静默消失");
    }

    #[test]
    fn identical_value_is_a_noop_not_a_history_churn() {
        let conn = mem();
        apply(&conn, "name", "许一", "rule", None, 0.7).unwrap();
        assert!(!apply(&conn, "name", "许一", "rule", None, 0.7).unwrap());
        let hist: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hist, 0);
    }

    #[test]
    fn manual_value_is_protected_from_auto_extraction() {
        let conn = mem();
        assert!(apply(&conn, "name", "许一（Jack）", "manual", None, 1.0).unwrap());
        // rule / model 都不许覆盖 manual。
        assert!(!apply(&conn, "name", "小明", "rule", None, 0.95).unwrap());
        assert!(!apply(&conn, "name", "小红", "model", None, 0.95).unwrap());
        let rows = get_all(&conn).unwrap();
        assert_eq!(rows[0].value, "许一（Jack）");
        // manual 覆盖 manual 是允许的（用户自己改自己的话）。
        assert!(apply(&conn, "name", "Jack", "manual", None, 1.0).unwrap());
        assert_eq!(get_all(&conn).unwrap()[0].value, "Jack");
    }

    #[test]
    fn auto_value_can_be_upgraded_by_manual_and_demoted_back_to_auto() {
        let conn = mem();
        apply(&conn, "tools", "Cursor", "rule", None, 0.7).unwrap();
        assert!(apply(&conn, "tools", "Cursor、Ollama", "manual", None, 1.0).unwrap());
        assert!(!apply(&conn, "tools", "VS Code", "model", None, 0.9).unwrap(), "manual 后自动抽取不再覆盖");
    }

    #[test]
    fn delete_archives_then_removes() {
        let conn = mem();
        apply(&conn, "city", "杭州", "rule", None, 0.9).unwrap();
        assert_eq!(delete(&conn, "city").unwrap(), 1);
        assert!(get_all(&conn).unwrap().is_empty());
        let hist: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile_history WHERE key = 'city'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hist, 1);
        // 再删一次：0 行，无害。
        assert_eq!(delete(&conn, "city").unwrap(), 0);
    }

    #[test]
    fn forget_all_clears_profile_and_history() {
        let conn = mem();
        apply(&conn, "name", "许一", "manual", None, 1.0).unwrap();
        apply(&conn, "name", "Jack", "manual", None, 1.0).unwrap(); // 产生一条 history
        db::forget_all(&conn).unwrap();
        assert!(get_all(&conn).unwrap().is_empty());
        let hist: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hist, 0, "「忘掉一切」包含忘了你是谁——history 也不留");
    }
}
