//! LLM 配置总线（spec-m1/spec-m2）：把模型选择从环境变量搬进 UI。
//!
//! 两个部件：
//! 1. **进程级覆盖层** `LLM_OVERRIDE`——`llm::current_client()` 每次调用都读它，
//!    切换即时生效，4 个调用点（对话/抽取/人格评估/日报）签名不动。
//!    优先级：覆盖层 > env（开发期习惯）> 默认值。
//! 2. **配置读写**——本地模型（`llm_models` / `llm_pull`）与云端条目
//!    （`llm_cloud`）共用 settings 表（不升 schema，§A3）。
//!
//! 序列化统一 camelCase（spec §A1）。

use std::sync::{OnceLock, RwLock};

use rusqlite::Connection;
use serde::Serialize;

use crate::llm::{DEFAULT_MODEL};
use crate::llm_cloud::{self, CloudEntry, CLOUD_PREFIX};
use crate::llm_models::{self, local_tag, probe_model, OllamaModelRow, LOCAL_PREFIX};
use crate::memory::db::{get_setting, set_setting};
use crate::memory::MemoryStore;

// ------------------------------------------------------------ 覆盖层

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    /// OpenAI 兼容端点（spec-m2）。条目在 `llm_cloud`，这里只存选择。
    Cloud,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Ollama => "ollama",
            Provider::Cloud => "cloud",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ollama" => Some(Provider::Ollama),
            "cloud" => Some(Provider::Cloud),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub provider: Provider,
    pub local_model: String,
    /// 本地形如 `ollama:<tag>`；云形如 `cloud:<entry-id>`。
    pub active_id: String,
}

static LLM_OVERRIDE: OnceLock<RwLock<Option<LlmConfig>>> = OnceLock::new();

fn cell() -> &'static RwLock<Option<LlmConfig>> {
    LLM_OVERRIDE.get_or_init(|| RwLock::new(None))
}

/// `OllamaClient::from_env()` 用：覆盖层有效时给模型 tag（仅 Ollama 腿）。
pub fn override_model() -> Option<String> {
    cell().read().ok()?.as_ref().and_then(|c| match c.provider {
        Provider::Ollama => Some(c.local_model.clone()),
        Provider::Cloud => None,
    })
}

pub fn current() -> Option<LlmConfig> {
    cell().read().ok()?.clone()
}

pub fn apply(cfg: LlmConfig) {
    if let Ok(mut w) = cell().write() {
        *w = Some(cfg);
    }
}

#[cfg(test)]
pub fn clear_for_test() {
    if let Ok(mut w) = cell().write() {
        *w = None;
    }
}

/// 覆盖层是全局静态——触它的测试必须串行（跨模块共用同一把锁）。
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn default_config() -> LlmConfig {
    LlmConfig {
        provider: Provider::Ollama,
        local_model: DEFAULT_MODEL.into(),
        active_id: format!("{LOCAL_PREFIX}{DEFAULT_MODEL}"),
    }
}

// ---------- settings 往返（spec §A3：llm_ 前缀，不升 schema）

const KEY_PROVIDER: &str = "llm_provider";
const KEY_LOCAL_MODEL: &str = "llm_local_model";
const KEY_ACTIVE_ID: &str = "llm_active_id";

pub fn persist(conn: &Connection, cfg: &LlmConfig) -> rusqlite::Result<()> {
    set_setting(conn, KEY_PROVIDER, cfg.provider.as_str())?;
    set_setting(conn, KEY_LOCAL_MODEL, &cfg.local_model)?;
    set_setting(conn, KEY_ACTIVE_ID, &cfg.active_id)?;
    Ok(())
}

/// settings → LlmConfig（缺键/空值回落默认，非法 provider 视为 ollama）。
pub fn read_settings(conn: &Connection) -> LlmConfig {
    let dflt = default_config();
    let provider = get_setting(conn, KEY_PROVIDER)
        .ok()
        .flatten()
        .as_deref()
        .and_then(Provider::parse)
        .unwrap_or(dflt.provider);
    let local_model = get_setting(conn, KEY_LOCAL_MODEL)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(dflt.local_model);
    let active_id = get_setting(conn, KEY_ACTIVE_ID)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{LOCAL_PREFIX}{local_model}"));
    LlmConfig { provider, local_model, active_id }
}

/// 当前生效配置：覆盖层优先，否则读 settings（命令层/回落逻辑共用）。
pub fn effective(conn: &Connection) -> LlmConfig {
    current().unwrap_or_else(|| read_settings(conn))
}

/// 启动回填：**配置过**（存在 llm_provider 键）才写覆盖层——从未配置过的
/// 用户保持 env / 默认行为不变。云端条目缓存同批刷新。
pub fn load_from_settings(conn: &Connection) -> Option<LlmConfig> {
    llm_cloud::refresh_cache(conn);
    if get_setting(conn, KEY_PROVIDER).ok().flatten().is_none() {
        return None;
    }
    let cfg = read_settings(conn);
    apply(cfg.clone());
    Some(cfg)
}

// ------------------------------------------------------------ 切换

/// 本地切换核心（校验 → 探测 → 落库 → 应用覆盖层）。
/// `installed` 与 `probe`/`persist` 由调用方注入，便于无网络单测。
/// 任何一步失败都不落库（持久化是最后一步）。
pub fn set_active_with(
    id: &str,
    installed: &[OllamaModelRow],
    probe: impl Fn(&str) -> Result<(), String>,
    persist: impl Fn(&LlmConfig) -> Result<(), String>,
) -> Result<LlmConfig, String> {
    let tag = local_tag(id)?;
    if !llm_models::is_installed(installed, tag) {
        return Err("model_not_installed".into());
    }
    probe(tag)?;
    let cfg = LlmConfig {
        provider: Provider::Ollama,
        local_model: tag.to_string(),
        active_id: format!("{LOCAL_PREFIX}{tag}"),
    };
    persist(&cfg)?;
    apply(cfg.clone());
    Ok(cfg)
}

/// 云端切换核心（spec-m2 §2.2）：条目存在 → 一枪测试 → 落库 → 应用。
/// 探测失败拒绝切换（不留「切了但不通」的中间态）。
pub fn set_active_cloud_with(
    id: &str,
    entries: &[CloudEntry],
    local_model: &str,
    probe: impl Fn(&CloudEntry) -> Result<(), String>,
    persist: impl Fn(&LlmConfig) -> Result<(), String>,
) -> Result<LlmConfig, String> {
    let entry_id = id
        .strip_prefix(CLOUD_PREFIX)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "unknown_id".to_string())?;
    let entry = entries
        .iter()
        .find(|e| e.id == entry_id)
        .ok_or_else(|| "entry_not_found".to_string())?;
    probe(entry)?;
    let cfg = LlmConfig {
        provider: Provider::Cloud,
        // 保留本地选择：删条目/切回来时立刻可用，不用用户再挑一次。
        local_model: local_model.to_string(),
        active_id: format!("{CLOUD_PREFIX}{entry_id}"),
    };
    persist(&cfg)?;
    apply(cfg.clone());
    Ok(cfg)
}

// ------------------------------------------------------------ 命令层

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigView {
    pub provider: String,
    pub local_model: String,
    pub active_id: String,
    pub sidecar_up: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked_key: Option<String>,
}

fn view_of(cfg: LlmConfig, sidecar_up: bool) -> LlmConfigView {
    // 云端只回脱敏值（spec-m2 §2.1 铁律）；本地腿没有 key。
    let masked_key = (cfg.provider == Provider::Cloud)
        .then(|| cfg.active_id.strip_prefix(CLOUD_PREFIX).map(str::to_string))
        .flatten()
        .and_then(|id| llm_cloud::find(&id))
        .map(|e| llm_cloud::mask_key(&e.key));
    LlmConfigView {
        provider: cfg.provider.as_str().into(),
        local_model: cfg.local_model,
        active_id: cfg.active_id,
        sidecar_up,
        masked_key,
    }
}

#[tauri::command]
pub fn llm_config_get(store: tauri::State<'_, MemoryStore>) -> Result<LlmConfigView, String> {
    let cfg = {
        let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
        effective(&conn)
    };
    let up = crate::llm::OllamaClient::new(llm_models::base_url(), DEFAULT_MODEL).is_up();
    Ok(view_of(cfg, up))
}

#[tauri::command]
pub fn llm_config_set_active(
    store: tauri::State<'_, MemoryStore>,
    id: String,
) -> Result<LlmConfigView, String> {
    if id.starts_with(CLOUD_PREFIX) {
        let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
        let entries = llm_cloud::load(&conn);
        let local_model = effective(&conn).local_model;
        let cfg = set_active_cloud_with(
            &id,
            &entries,
            &local_model,
            llm_cloud::probe_entry,
            |cfg| persist(&conn, cfg).map_err(|e| e.to_string()),
        )?;
        return Ok(view_of(cfg, true));
    }

    let installed = llm_models::fetch_installed()?;
    let base = llm_models::base_url();
    let cfg = set_active_with(
        &id,
        &installed,
        |tag| probe_model(&base, tag),
        |cfg| {
            let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
            persist(&conn, cfg).map_err(|e| e.to_string())
        },
    )?;
    Ok(view_of(cfg, true))
}

// ------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::memory::db::init_schema_for_test(&conn).unwrap();
        conn
    }

    fn installed(names: &[&str]) -> Vec<OllamaModelRow> {
        names
            .iter()
            .map(|n| OllamaModelRow { name: (*n).into(), size_bytes: 1, modified: String::new() })
            .collect()
    }

    fn entry(id: &str) -> CloudEntry {
        CloudEntry {
            id: id.into(),
            name: "DeepSeek".into(),
            kind: llm_cloud::EntryKind::Custom,
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
            key: "sk-secret-1234".into(),
        }
    }

    // ---------- 覆盖层（3）

    #[test]
    fn override_beats_env_and_clear_falls_back() {
        let _g = test_lock();
        clear_for_test();
        apply(LlmConfig {
            provider: Provider::Ollama,
            local_model: "gemma3:4b".into(),
            active_id: "ollama:gemma3:4b".into(),
        });
        // from_env：覆盖层 > env（测试环境未设 BONSAI_LLM_MODEL）> 默认
        assert_eq!(crate::llm::OllamaClient::from_env().model(), "gemma3:4b");
        assert_eq!(override_model().as_deref(), Some("gemma3:4b"));

        clear_for_test();
        assert_eq!(override_model(), None);
        assert_eq!(crate::llm::OllamaClient::from_env().model(), DEFAULT_MODEL);
    }

    #[test]
    fn cloud_override_yields_no_local_model_override() {
        let _g = test_lock();
        clear_for_test();
        apply(LlmConfig {
            provider: Provider::Cloud,
            local_model: "qwen3.5:2b".into(),
            active_id: "cloud:c1".into(),
        });
        // 云端激活时本地腿回落到 env/默认，不能被云端条目污染。
        assert_eq!(override_model(), None);
        assert_eq!(crate::llm::OllamaClient::from_env().model(), DEFAULT_MODEL);
        clear_for_test();
    }

    #[test]
    fn startup_backfill_only_when_configured() {
        let _g = test_lock();
        let conn = mem_conn();
        clear_for_test();
        // 从未配置 → 不写覆盖层（env/默认行为不变）
        assert!(load_from_settings(&conn).is_none());
        assert_eq!(current(), None);

        let cfg = LlmConfig {
            provider: Provider::Ollama,
            local_model: "llama3.2:3b".into(),
            active_id: "ollama:llama3.2:3b".into(),
        };
        persist(&conn, &cfg).unwrap();
        assert_eq!(load_from_settings(&conn), Some(cfg.clone()));
        assert_eq!(current(), Some(cfg));
        clear_for_test();
    }

    // ---------- settings（3）

    #[test]
    fn settings_roundtrip_persists_three_keys() {
        let conn = mem_conn();
        let cfg = LlmConfig {
            provider: Provider::Ollama,
            local_model: "qwen3.5:4b".into(),
            active_id: "ollama:qwen3.5:4b".into(),
        };
        persist(&conn, &cfg).unwrap();
        assert_eq!(get_setting(&conn, KEY_PROVIDER).unwrap().as_deref(), Some("ollama"));
        assert_eq!(get_setting(&conn, KEY_LOCAL_MODEL).unwrap().as_deref(), Some("qwen3.5:4b"));
        assert_eq!(get_setting(&conn, KEY_ACTIVE_ID).unwrap().as_deref(), Some("ollama:qwen3.5:4b"));
        assert_eq!(read_settings(&conn), cfg);
    }

    #[test]
    fn read_settings_defaults_when_keys_missing() {
        let conn = mem_conn();
        let cfg = read_settings(&conn);
        assert_eq!(cfg.provider, Provider::Ollama);
        assert_eq!(cfg.local_model, DEFAULT_MODEL);
        assert_eq!(cfg.active_id, format!("ollama:{DEFAULT_MODEL}"));
    }

    #[test]
    fn cloud_config_roundtrips_through_settings() {
        let conn = mem_conn();
        let cfg = LlmConfig {
            provider: Provider::Cloud,
            local_model: "qwen3.5:2b".into(),
            active_id: "cloud:c1".into(),
        };
        persist(&conn, &cfg).unwrap();
        assert_eq!(read_settings(&conn), cfg);
        assert_eq!(get_setting(&conn, KEY_PROVIDER).unwrap().as_deref(), Some("cloud"));
    }

    // ---------- 本地切换（3）

    #[test]
    fn set_active_rejects_uninstalled_tag() {
        let _g = test_lock();
        clear_for_test();
        let persisted = std::cell::Cell::new(0);
        let r = set_active_with(
            "ollama:not-there",
            &installed(&["qwen3.5:2b"]),
            |_| Ok(()),
            |_| {
                persisted.set(persisted.get() + 1);
                Ok(())
            },
        );
        assert_eq!(r.unwrap_err(), "model_not_installed");
        assert_eq!(persisted.get(), 0, "校验失败不得落库");
        assert_eq!(current(), None, "校验失败不得改覆盖层");
    }

    #[test]
    fn set_active_success_writes_three_keys_and_applies() {
        let _g = test_lock();
        let conn = mem_conn();
        clear_for_test();
        let cfg = set_active_with(
            "ollama:qwen3.5:4b",
            &installed(&["qwen3.5:2b", "qwen3.5:4b"]),
            |tag| {
                assert_eq!(tag, "qwen3.5:4b");
                Ok(())
            },
            |c| persist(&conn, c).map_err(|e| e.to_string()),
        )
        .unwrap();
        assert_eq!(cfg.local_model, "qwen3.5:4b");
        assert_eq!(cfg.active_id, "ollama:qwen3.5:4b");
        assert_eq!(read_settings(&conn), cfg);
        assert_eq!(current(), Some(cfg));
        clear_for_test();
    }

    #[test]
    fn set_active_probe_failure_does_not_persist() {
        let _g = test_lock();
        let conn = mem_conn();
        clear_for_test();
        let r = set_active_with(
            "ollama:qwen3.5:4b",
            &installed(&["qwen3.5:4b"]),
            |_| Err("probe_failed: timeout".into()),
            |c| persist(&conn, c).map_err(|e| e.to_string()),
        );
        assert_eq!(r.unwrap_err(), "probe_failed: timeout");
        assert!(get_setting(&conn, KEY_PROVIDER).unwrap().is_none(), "探测失败不得落库");
    }

    // ---------- 云端切换（3）

    #[test]
    fn cloud_switch_keeps_local_model_and_applies() {
        let _g = test_lock();
        let conn = mem_conn();
        clear_for_test();
        let entries = vec![entry("c1")];
        let cfg = set_active_cloud_with(
            "cloud:c1",
            &entries,
            "gemma3:4b",
            |e| {
                assert_eq!(e.id, "c1");
                Ok(())
            },
            |c| persist(&conn, c).map_err(|e| e.to_string()),
        )
        .unwrap();
        assert_eq!(cfg.provider, Provider::Cloud);
        assert_eq!(cfg.active_id, "cloud:c1");
        assert_eq!(cfg.local_model, "gemma3:4b", "回落到本地时用回这个选择");
        assert_eq!(read_settings(&conn), cfg);
        assert_eq!(current(), Some(cfg));
        clear_for_test();
    }

    #[test]
    fn cloud_switch_rejects_missing_entry_and_failed_probe() {
        let _g = test_lock();
        let conn = mem_conn();
        clear_for_test();
        let entries = vec![entry("c1")];

        let missing = set_active_cloud_with(
            "cloud:nope",
            &entries,
            "qwen3.5:2b",
            |_| Ok(()),
            |c| persist(&conn, c).map_err(|e| e.to_string()),
        );
        assert_eq!(missing.unwrap_err(), "entry_not_found");

        let failed = set_active_cloud_with(
            "cloud:c1",
            &entries,
            "qwen3.5:2b",
            |_| Err("test_failed: 401".into()),
            |c| persist(&conn, c).map_err(|e| e.to_string()),
        );
        assert_eq!(failed.unwrap_err(), "test_failed: 401");
        assert!(get_setting(&conn, KEY_PROVIDER).unwrap().is_none(), "测试失败不得落库");
        assert_eq!(current(), None);
    }

    #[test]
    fn cloud_switch_rejects_malformed_id() {
        let _g = test_lock();
        clear_for_test();
        let r = set_active_cloud_with("cloud:", &[], "m", |_| Ok(()), |_| Ok(()));
        assert_eq!(r.unwrap_err(), "unknown_id");
    }

    // ---------- 视图脱敏（1）

    #[test]
    fn view_masks_cloud_key_and_hides_local_key() {
        let _g = test_lock();
        llm_cloud::set_cache_for_test(vec![entry("c1")]);
        let cloud = view_of(
            LlmConfig {
                provider: Provider::Cloud,
                local_model: "qwen3.5:2b".into(),
                active_id: "cloud:c1".into(),
            },
            true,
        );
        assert_eq!(cloud.masked_key.as_deref(), Some("••••1234"));
        // 序列化后就这一份脱敏值，没有明文
        let json = serde_json::to_string(&cloud).unwrap();
        assert!(!json.contains("sk-secret"));

        let local = view_of(
            LlmConfig {
                provider: Provider::Ollama,
                local_model: "qwen3.5:2b".into(),
                active_id: "ollama:qwen3.5:2b".into(),
            },
            true,
        );
        assert_eq!(local.masked_key, None);
        llm_cloud::set_cache_for_test(Vec::new());
    }
}
