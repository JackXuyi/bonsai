//! 本地模型清单 + 推荐目录 + 删除（spec-m1，拆自 `llm_admin.rs`）。
//!
//! 这里只碰 sidecar 的 HTTP API（`/api/tags`）与 CLI（`ollama rm`）。
//! 下载子进程在 `llm_pull.rs`，配置总线在 `llm_admin.rs`。

use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

use crate::llm::{serve, DEFAULT_BASE_URL};
use crate::memory::MemoryStore;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaModelRow {
    pub name: String,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(serde::Deserialize)]
struct TagsResp {
    #[serde(default)]
    models: Vec<TagRow>,
}

#[derive(serde::Deserialize)]
struct TagRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: String,
}

pub fn base_url() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_BASE_URL.into())
}

/// 代理 sidecar `GET /api/tags`。不可达 → Err("sidecar_down")，前端回退静态
/// 清单 + 灰点（绝不返回空列表冒充「没装模型」）。
pub fn fetch_installed() -> Result<Vec<OllamaModelRow>, String> {
    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build()
        .get(&format!("{}/api/tags", base_url()))
        .call()
        .map_err(|_| "sidecar_down".to_string())?;
    let body: TagsResp = resp.into_json().map_err(|_| "sidecar_down".to_string())?;
    Ok(body
        .models
        .into_iter()
        .map(|m| OllamaModelRow { name: m.name, size_bytes: m.size, modified: m.modified_at })
        .collect())
}

/// tag 比对：`qwen3.5:2b` == `qwen3.5:2b`；`gemma3` == `gemma3:latest`。
pub fn same_model(a: &str, b: &str) -> bool {
    fn bare(s: &str) -> &str {
        s.strip_suffix(":latest").unwrap_or(s)
    }
    !a.is_empty() && bare(a) == bare(b)
}

pub fn is_installed(list: &[OllamaModelRow], tag: &str) -> bool {
    list.iter().any(|m| same_model(&m.name, tag))
}

pub(crate) const LOCAL_PREFIX: &str = "ollama:";

/// `ollama:<tag>` → tag（其它形态 → "unknown_id"）。
pub(crate) fn local_tag(id: &str) -> Result<&str, String> {
    id.strip_prefix(LOCAL_PREFIX)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "unknown_id".to_string())
}

/// 1-token 测试请求：模型能加载并回一句话才算切换成功（冷加载留足 90s）。
pub(crate) fn probe_model(base: &str, tag: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "model": tag,
        "messages": [{ "role": "user", "content": "hi" }],
        "stream": false,
        "think": false,
        "options": { "num_predict": 1 }
    });
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(90))
        .build()
        .post(&format!("{base}/api/chat"))
        .send_json(body)
        .map(|_| ())
        .map_err(|e| format!("probe_failed: {e}"))
}

// ------------------------------------------------------------ 推荐目录

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRow {
    pub tag: &'static str,
    pub size_label: &'static str,
    pub size_bytes: u64,
    pub note: &'static str,
    pub think_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_hint: Option<&'static str>,
    pub recommended: bool,
}

/// 编译内置目录（纯数据）。think_ok=false 的模型（思维链关不掉，正文会被
/// 推理过程污染——qwen3:4b 的教训）一律 recommended=false，前端折叠进「更多」。
pub fn catalog() -> &'static [CatalogRow] {
    &[
        CatalogRow {
            tag: "qwen3.5:2b",
            size_label: "0.8GB",
            size_bytes: 800_000_000,
            note: "阿盆默认 · 中文稳",
            think_ok: true,
            ram_hint: None,
            recommended: true,
        },
        CatalogRow {
            tag: "qwen3.5:4b",
            size_label: "2.4GB",
            size_bytes: 2_400_000_000,
            note: "同族 · 质量更好",
            think_ok: true,
            ram_hint: Some("16GB+"),
            recommended: true,
        },
        CatalogRow {
            tag: "gemma3:4b",
            size_label: "3.3GB",
            size_bytes: 3_300_000_000,
            note: "Google · 中英均可",
            think_ok: true,
            ram_hint: Some("16GB+"),
            recommended: true,
        },
        CatalogRow {
            tag: "gemma3:1b",
            size_label: "0.8GB",
            size_bytes: 800_000_000,
            note: "Google · 轻量兜底",
            think_ok: true,
            ram_hint: None,
            recommended: false,
        },
        CatalogRow {
            tag: "llama3.2:3b",
            size_label: "2.0GB",
            size_bytes: 2_000_000_000,
            note: "Meta · 中英混合稍弱",
            think_ok: true,
            ram_hint: None,
            recommended: false,
        },
        CatalogRow {
            tag: "llama3.1:8b",
            size_label: "4.9GB",
            size_bytes: 4_900_000_000,
            note: "Meta · 8B 质量档",
            think_ok: true,
            ram_hint: Some("16GB+"),
            recommended: false,
        },
        CatalogRow {
            tag: "qwen3:8b",
            size_label: "5.2GB",
            size_bytes: 5_200_000_000,
            note: "⚠ 思维链不可关 · qwen3:4b 同款问题",
            think_ok: false,
            ram_hint: Some("16GB+"),
            recommended: false,
        },
        CatalogRow {
            tag: "deepseek-r1:1.5b",
            size_label: "1.1GB",
            size_bytes: 1_100_000_000,
            note: "⚠ 思维链不可关 · 会污染正文",
            think_ok: false,
            ram_hint: None,
            recommended: false,
        },
        CatalogRow {
            tag: "deepseek-r1:8b",
            size_label: "4.9GB",
            size_bytes: 4_900_000_000,
            note: "DeepSeek · 思维链不可关 · 会污染正文",
            think_ok: false,
            ram_hint: Some("16GB+"),
            recommended: false,
        },
        CatalogRow {
            tag: "deepseek-r1:14b",
            size_label: "9.0GB",
            size_bytes: 9_000_000_000,
            note: "DeepSeek · 蒸馏 14B · 思维链不可关",
            think_ok: false,
            ram_hint: Some("32GB+"),
            recommended: false,
        },
    ]
}

// ------------------------------------------------------------ 命令层

#[tauri::command]
pub fn llm_model_list_installed() -> Result<Vec<OllamaModelRow>, String> {
    fetch_installed()
}

#[tauri::command]
pub fn llm_catalog() -> Vec<CatalogRow> {
    catalog().to_vec()
}

#[tauri::command]
pub fn llm_model_remove(
    store: tauri::State<'_, MemoryStore>,
    tag: String,
) -> Result<(), String> {
    let cfg = {
        let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
        crate::llm_admin::effective(&conn)
    };
    if same_model(&cfg.local_model, &tag)
        || same_model(cfg.active_id.trim_start_matches(LOCAL_PREFIX), &tag)
    {
        return Err("model_in_use".into());
    }
    let bin = serve::sidecar_path().ok_or_else(|| "sidecar_missing".to_string())?;
    let status = Command::new(bin)
        .arg("rm")
        .arg(&tag)
        .env("OLLAMA_HOST", format!("127.0.0.1:{}", serve::PORT))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("spawn_failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("remove_failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_model_handles_latest_suffix() {
        assert!(same_model("qwen3.5:2b", "qwen3.5:2b"));
        assert!(same_model("gemma3:latest", "gemma3"));
        assert!(!same_model("gemma3:4b", "gemma3"));
        assert!(!same_model("", ""));
    }

    #[test]
    fn local_tag_rejects_unknown_id_shapes() {
        assert_eq!(local_tag("ollama:qwen3.5:2b").unwrap(), "qwen3.5:2b");
        assert!(local_tag("cloud:abc").is_err());
        assert!(local_tag("ollama:").is_err());
        assert!(local_tag("").is_err());
    }

    #[test]
    fn installed_lookup_matches_by_tag() {
        let list = vec![OllamaModelRow {
            name: "qwen3.5:2b".into(),
            size_bytes: 1,
            modified: String::new(),
        }];
        assert!(is_installed(&list, "qwen3.5:2b"));
        assert!(!is_installed(&list, "gemma3:4b"));
    }

    #[test]
    fn catalog_rows_are_complete() {
        for row in catalog() {
            assert!(!row.tag.is_empty());
            assert!(!row.size_label.is_empty());
            assert!(row.size_bytes > 0);
            assert!(!row.note.is_empty());
        }
        assert!(
            catalog().iter().any(|r| r.tag == crate::llm::DEFAULT_MODEL),
            "默认模型必须在目录里"
        );
    }

    #[test]
    fn non_think_ok_models_are_never_recommended() {
        for row in catalog() {
            if !row.think_ok {
                assert!(!row.recommended, "{} 思维链不可关，不能推荐", row.tag);
            }
        }
    }
}
