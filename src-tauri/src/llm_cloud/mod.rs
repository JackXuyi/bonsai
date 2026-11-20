//! 云端模型清单（spec-m2 §2）：多实例 CRUD + 脱敏 + 一枪测试。
//!
//! 存储：settings 表 key `llm_cloud_entries` = JSON 数组（条目量个位数，
//! 不建新表、不升 schema，§A3）。缓存与读写往返拆在 `store`，命令层拆在
//! `commands`（单文件 500 行上限）。
//!
//! 两条铁律：
//! 1. **任何读取路径都只返回 `mask_key()`**——明文 key 只出现在写入路径与
//!    进程内的 `CloudClient` / `AnthropicClient` 里（spec-m2 §4.4）。
//! 2. **条目缓存在进程级**（`store::CACHE`）：模型腿取 client 时不能去抢 DB
//!    锁，启动回填 + 每次写操作后刷新。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::llm::{
    AnthropicClient, CloudClient, ANTHROPIC_BASE_URL, CLOUD_TEST_TIMEOUT_SECS, OPENAI_BASE_URL,
};

mod commands;
mod store;

pub use commands::*;
pub use store::{entries, find, load, refresh_cache};
#[cfg(test)]
pub use store::set_cache_for_test;
use store::save_all;

pub const CLOUD_PREFIX: &str = "cloud:";

/// 条目类型。`openai` / `claude` 的官方端点锁定到常量，用户不用填地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    #[serde(rename = "custom")]
    Custom,
    #[serde(rename = "openai")]
    OpenAi,
    /// Claude 原生协议（spec-m2.1）
    #[serde(rename = "claude")]
    Claude,
}

/// 落库形态（含明文 key —— 只在写入路径与进程内存中出现）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudEntry {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub base_url: String,
    pub model: String,
    pub key: String,
}

/// 前端提交的表单（`id` 存在 = 编辑；`key` 留空 = 保持原 key）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudEntryInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub kind: EntryKind,
    #[serde(default)]
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub key: String,
}

/// 对外形态：key 只给脱敏值。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudEntryView {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub base_url: String,
    pub model: String,
    pub masked_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub latency_ms: u64,
}

/// `sk-1234567890abcd` → `••••abcd`；≤4 位全遮；空串保持空。
pub fn mask_key(key: &str) -> String {
    let k = key.trim();
    if k.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = k.chars().collect();
    if chars.len() <= 4 {
        return "••••".into();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("••••{tail}")
}

impl CloudEntry {
    fn view(&self) -> CloudEntryView {
        CloudEntryView {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            masked_key: mask_key(&self.key),
        }
    }
}

// ------------------------------------------------------------ 校验

/// `http(s)://host...`，无空白。云端端点不接受别的协议（file:// 之类）。
pub fn valid_base_url(url: &str) -> bool {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"));
    match rest {
        Some(r) => !r.is_empty() && !r.starts_with('/') && !r.contains(|c: char| c.is_whitespace()),
        None => false,
    }
}

fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("c{nanos:x}{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 表单 → 条目（不落库，纯函数）。错误码给前端 humanize。
pub fn build_entry(
    input: &CloudEntryInput,
    existing: &[CloudEntry],
) -> Result<CloudEntry, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("name_required".into());
    }
    let model = input.model.trim();
    if model.is_empty() {
        return Err("model_required".into());
    }

    // 官方端点是常量，不接受用户填的地址（填了也不采信）。
    let base_url = match input.kind {
        EntryKind::OpenAi => OPENAI_BASE_URL.to_string(),
        EntryKind::Claude => ANTHROPIC_BASE_URL.to_string(),
        EntryKind::Custom => {
            let url = input.base_url.trim().trim_end_matches('/').to_string();
            if !valid_base_url(&url) {
                return Err("bad_url".into());
            }
            url
        }
    };

    let prev = input
        .id
        .as_ref()
        .and_then(|id| existing.iter().find(|e| &e.id == id));
    if input.id.is_some() && prev.is_none() {
        return Err("entry_not_found".into());
    }

    // 编辑时留空 = 保持原 key；新增时必填。
    let key = match (input.key.trim(), prev) {
        ("", Some(p)) => p.key.clone(),
        ("", None) => return Err("key_required".into()),
        (k, _) => k.to_string(),
    };

    Ok(CloudEntry {
        id: input.id.clone().unwrap_or_else(new_id),
        name: name.to_string(),
        kind: input.kind,
        base_url,
        model: model.to_string(),
        key,
    })
}

// ------------------------------------------------------------ 写操作

/// 保存（新增 / 编辑）。返回脱敏视图——调用方拿不到明文。
pub fn save_entry(conn: &Connection, input: CloudEntryInput) -> Result<CloudEntryView, String> {
    let mut list = load(conn);
    let entry = build_entry(&input, &list)?;
    match list.iter().position(|e| e.id == entry.id) {
        Some(i) => list[i] = entry.clone(),
        None => list.push(entry.clone()),
    }
    save_all(conn, &list)?;
    Ok(entry.view())
}

/// 删除。激活条目被删 → 联动回落本地默认模型（防锁死，spec-m2 §2.2）。
pub fn remove_entry(conn: &Connection, id: &str) -> Result<(), String> {
    let mut list = load(conn);
    list.retain(|e| e.id != id);
    save_all(conn, &list)?;

    let cfg = crate::llm_admin::effective(conn);
    if cfg.active_id == format!("{CLOUD_PREFIX}{id}") {
        let next = crate::llm_admin::LlmConfig {
            provider: crate::llm_admin::Provider::Ollama,
            active_id: format!("ollama:{}", cfg.local_model),
            local_model: cfg.local_model,
        };
        crate::llm_admin::persist(conn, &next).map_err(|e| e.to_string())?;
        crate::llm_admin::apply(next);
        log::info!("active cloud entry `{id}` removed — fell back to the local model");
    }
    Ok(())
}

/// 探测一个条目：成功 = 端点认这个地址 + key + 模型名。
/// 协议跟着 kind 走——Claude 用 `x-api-key` + `/v1/messages`，其余走
/// OpenAI 兼容的 `/chat/completions`（spec-m2.1 §1）。
pub fn probe_entry(entry: &CloudEntry) -> Result<(), String> {
    let result = match entry.kind {
        EntryKind::Claude => AnthropicClient::new(
            entry.base_url.clone(),
            entry.model.clone(),
            entry.key.clone(),
        )
        .ping(CLOUD_TEST_TIMEOUT_SECS),
        _ => CloudClient::new(entry.base_url.clone(), entry.model.clone(), entry.key.clone())
            .ping(CLOUD_TEST_TIMEOUT_SECS),
    };
    result.map_err(|e| format!("test_failed: {e}"))
}

/// 校验错误码 → 一句人话（命令层直接给前端展示）。
pub fn humanize(code: String) -> String {
    match code.as_str() {
        "name_required" => "名字不能空".into(),
        "model_required" => "模型名不能空".into(),
        "key_required" => "API Key 不能空".into(),
        "bad_url" => "接口地址要是 http(s) 开头的完整地址".into(),
        "entry_not_found" => "这条配置不在了，刷新一下".into(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db::{get_setting, init_schema_for_test};

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema_for_test(&conn).unwrap();
        conn
    }

    fn input(name: &str, url: &str, key: &str) -> CloudEntryInput {
        CloudEntryInput {
            id: None,
            name: name.into(),
            kind: EntryKind::Custom,
            base_url: url.into(),
            model: "deepseek-chat".into(),
            key: key.into(),
        }
    }

    // ---------- mask_key（3）

    #[test]
    fn mask_key_keeps_last_four() {
        assert_eq!(mask_key("sk-1234567890abcd"), "••••abcd");
        assert_eq!(mask_key("  sk-live-9999  "), "••••9999");
    }

    #[test]
    fn mask_key_hides_short_keys_entirely() {
        assert_eq!(mask_key("abcd"), "••••");
        assert_eq!(mask_key("ab"), "••••");
    }

    #[test]
    fn mask_key_of_empty_is_empty() {
        assert_eq!(mask_key(""), "");
        assert_eq!(mask_key("   "), "");
    }

    // ---------- 校验（6）

    #[test]
    fn save_assigns_id_and_persists_masked_view() {
        let _g = crate::llm_admin::test_lock(); // 进程级缓存是全局的
        let conn = mem_conn();
        let view = save_entry(&conn, input("DeepSeek", "https://api.deepseek.com/v1", "sk-abcdefgh")).unwrap();
        assert!(!view.id.is_empty());
        assert_eq!(view.masked_key, "••••efgh", "对外只给脱敏值");
        let stored = load(&conn);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key, "sk-abcdefgh", "落库保留明文（用于构造 client）");
        assert_eq!(entries().len(), 1, "写操作必须刷新进程缓存");
        assert!(get_setting(&conn, "llm_cloud_entries").unwrap().unwrap().contains("sk-abcdefgh"));
    }

    #[test]
    fn edit_keeps_id_and_empty_key_keeps_old_secret() {
        let _g = crate::llm_admin::test_lock();
        let conn = mem_conn();
        let first = save_entry(&conn, input("旧名", "https://a/v1", "sk-first-key")).unwrap();
        let edited = save_entry(
            &conn,
            CloudEntryInput {
                id: Some(first.id.clone()),
                name: "新名".into(),
                kind: EntryKind::Custom,
                base_url: "https://b/v1".into(),
                model: "m2".into(),
                key: "".into(),
            },
        )
        .unwrap();
        assert_eq!(edited.id, first.id);
        assert_eq!(edited.name, "新名");
        let stored = load(&conn);
        assert_eq!(stored.len(), 1, "编辑不得新增条目");
        assert_eq!(stored[0].key, "sk-first-key", "留空 = 保持原 key");
    }

    #[test]
    fn official_kinds_force_their_endpoint() {
        let _g = crate::llm_admin::test_lock();
        let conn = mem_conn();
        let openai = save_entry(
            &conn,
            CloudEntryInput {
                id: None,
                name: "官方".into(),
                kind: EntryKind::OpenAi,
                base_url: "https://evil.example/v1".into(),
                model: "gpt-4o-mini".into(),
                key: "sk-x".into(),
            },
        )
        .unwrap();
        assert_eq!(openai.base_url, OPENAI_BASE_URL);

        // spec-m2.1：Claude 官方端点同样锁定，用户填的地址不采信。
        let claude = save_entry(
            &conn,
            CloudEntryInput {
                id: None,
                name: "Claude".into(),
                kind: EntryKind::Claude,
                base_url: "https://proxy.example".into(),
                model: "claude-3-5-sonnet-latest".into(),
                key: "sk-ant-x".into(),
            },
        )
        .unwrap();
        assert_eq!(claude.base_url, ANTHROPIC_BASE_URL);
        assert_eq!(claude.kind, EntryKind::Claude);
    }

    #[test]
    fn invalid_url_and_empty_fields_are_rejected() {
        let _g = crate::llm_admin::test_lock();
        let conn = mem_conn();
        let bad = |i: CloudEntryInput| save_entry(&conn, i).unwrap_err();
        assert_eq!(bad(input("n", "api.deepseek.com/v1", "k")), "bad_url");
        assert_eq!(bad(input("n", "file:///etc/passwd", "k")), "bad_url");
        assert_eq!(bad(input("n", "https://a b/v1", "k")), "bad_url");
        assert_eq!(bad(input("  ", "https://a/v1", "k")), "name_required");
        assert_eq!(
            bad(CloudEntryInput { model: " ".into(), ..input("n", "https://a/v1", "k") }),
            "model_required"
        );
        assert_eq!(bad(input("n", "https://a/v1", " ")), "key_required");
        assert_eq!(load(&conn).len(), 0, "校验失败不得落库");
    }

    #[test]
    fn save_with_unknown_id_is_rejected() {
        let _g = crate::llm_admin::test_lock();
        let conn = mem_conn();
        let err = save_entry(
            &conn,
            CloudEntryInput { id: Some("nope".into()), ..input("n", "https://a/v1", "k") },
        )
        .unwrap_err();
        assert_eq!(err, "entry_not_found");
    }

    // ---------- 删除回落（2）

    #[test]
    fn removing_active_entry_falls_back_to_local() {
        use crate::llm_admin::{self, LlmConfig, Provider};
        let _g = llm_admin::test_lock();
        llm_admin::clear_for_test();
        let conn = mem_conn();

        let view = save_entry(&conn, input("DeepSeek", "https://a/v1", "sk-key-1234")).unwrap();
        let cfg = LlmConfig {
            provider: Provider::Cloud,
            local_model: "qwen3.5:2b".into(),
            active_id: format!("cloud:{}", view.id),
        };
        llm_admin::persist(&conn, &cfg).unwrap();
        llm_admin::apply(cfg);

        remove_entry(&conn, &view.id).unwrap();
        assert_eq!(load(&conn).len(), 0);
        let after = llm_admin::effective(&conn);
        assert_eq!(after.provider, Provider::Ollama);
        assert_eq!(after.active_id, "ollama:qwen3.5:2b");
        assert_eq!(llm_admin::current(), Some(after), "覆盖层必须同步回落");
        llm_admin::clear_for_test();
    }

    #[test]
    fn removing_inactive_entry_leaves_config_alone() {
        use crate::llm_admin::{self, LlmConfig, Provider};
        let _g = llm_admin::test_lock();
        llm_admin::clear_for_test();
        let conn = mem_conn();
        let view = save_entry(&conn, input("x", "https://a/v1", "sk-key-1234")).unwrap();
        let cfg = LlmConfig {
            provider: Provider::Ollama,
            local_model: "gemma3:4b".into(),
            active_id: "ollama:gemma3:4b".into(),
        };
        llm_admin::persist(&conn, &cfg).unwrap();
        llm_admin::apply(cfg.clone());

        remove_entry(&conn, &view.id).unwrap();
        assert_eq!(llm_admin::effective(&conn), cfg);
        llm_admin::clear_for_test();
    }

    // ---------- 脱敏铁律（1）

    #[test]
    fn every_read_path_masks_the_key() {
        let _g = crate::llm_admin::test_lock();
        let conn = mem_conn();
        save_entry(&conn, input("DeepSeek", "https://a/v1", "sk-super-secret-9999")).unwrap();
        let views: Vec<CloudEntryView> = load(&conn).iter().map(CloudEntry::view).collect();
        let json = serde_json::to_string(&views).unwrap();
        assert!(!json.contains("sk-super-secret"), "视图 JSON 不得含明文 key");
        assert!(json.contains("••••9999"));
        assert!(!json.contains("\"key\""), "视图结构里没有 key 字段");
    }

    // ---------- kind 序列化（1，spec-m2.1）

    #[test]
    fn claude_kind_round_trips_as_lowercase() {
        // 落库 JSON 里的 kind 是前端唯一能读到的枚举；写成 "Claude" 会让
        // CloudEntryCard 的标签表查不到值。
        let json = serde_json::to_string(&EntryKind::Claude).unwrap();
        assert_eq!(json, "\"claude\"");
        let back: EntryKind = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(back, EntryKind::Claude);
    }
}
