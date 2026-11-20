//! LLM access — local-first (decision 2026-11-18, supersedes hybrid routing).
//!
//! One client for both model legs, per plan-memory §5.2 / plan-persona §4.2:
//! - dialogue: persona card injected as the system prompt (`LocalBackend`)
//! - extraction/consolidation: structured-JSON replies (`LocalExtractor`)
//!
//! Three transports behind one trait (`ChatClient`):
//! - `ollama::OllamaClient` — the bundled sidecar (`bundle.externalBin`), owned
//!   by the app: spawned at startup on port 11435 (never 11434 — that belongs
//!   to a system-wide Ollama, if any), stopped on exit, models stored in
//!   project-local `.llm/models` during dev and app data at runtime.
//! - `cloud::CloudClient` — an OpenAI-compatible `/chat/completions` endpoint.
//! - `anthropic::AnthropicClient` — Claude's native `/v1/messages` (spec-m2.1).
//!
//! The two cloud legs are only ever used after the user adds an entry AND
//! confirms the opt-in warning (spec-m2 §4). Default stays on-device: nothing
//! leaves the machine unless a cloud entry is active.
//!
//! `current_client()` is the single factory the four call sites use; it reads
//! the override layer (`llm_admin`) so a switch takes effect without restart.
//!
//! Config via env (still honoured when no UI config exists):
//! - `OLLAMA_HOST` — base URL, default `http://127.0.0.1:11435`
//! - `BONSAI_LLM_MODEL` — model tag, default `qwen3.5:2b`

use std::sync::Arc;

pub mod anthropic;
pub mod cloud;
pub mod ollama;
pub mod serve;

pub use anthropic::{parse_anthropic_sse_line, AnthropicClient};
pub use cloud::{parse_sse_line, CloudClient};
pub use ollama::{parse_stream_line, OllamaClient};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11435";
pub const DEFAULT_MODEL: &str = "qwen3.5:2b";
/// One dialogue turn. Long enough for a cold-loaded 7B model on Apple Silicon.
pub const CHAT_TIMEOUT_SECS: u64 = 120;
/// One extraction pass over a day's utterances (plan §5.2: on timeout the
/// consolidate pipeline falls back to `RuleExtractor` and keeps the events
/// unconsumed — no loss, retry next run).
pub const EXTRACT_TIMEOUT_SECS: u64 = 30;
/// Liveness probe.
pub const PROBE_TIMEOUT_SECS: u64 = 2;
/// Cloud one-shot test ("测试连接" + switch probe). One 1-token request.
pub const CLOUD_TEST_TIMEOUT_SECS: u64 = 30;
/// Official OpenAI endpoint — `kind = openai` locks the base URL to this.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Official Claude endpoint — `kind = claude` locks the base URL to this.
/// The native transport appends `/v1/messages` (spec-m2.1 §1).
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
/// Required by every `/v1/messages` call; pinning it keeps the body shape
/// valid instead of silently depending on the provider's default.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Cloud replies are capped: the companion never needs a long essay, and an
/// uncapped stream is how you get a 40k-token bill from one typo.
pub const CLOUD_MAX_TOKENS: u32 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http transport error: {0}")]
    Transport(String),
    #[error("server returned {0}")]
    Status(u16),
    #[error("response decode failed: {0}")]
    Decode(String),
}

/// The model leg, transport-agnostic (spec-m2 §1.1). `DialogueBackend` sits
/// one layer above this and is untouched: swapping the transport must not
/// change how the persona card is rendered or judged.
pub trait ChatClient: Send + Sync {
    fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError>;
    fn chat_stream(
        &self,
        messages: &[Msg],
        temperature: f64,
        timeout: u64,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, LlmError>;
    fn is_up(&self) -> bool;

    /// 日志用的身份标签（`ollama:qwen3.5:2b` / `cloud:deepseek-chat`）。
    /// 不是 spec 的三方法之一，但 consolidate / 对话日志需要说清「谁回的」，
    /// 且 trait object 无法 downcast。
    fn label(&self) -> String {
        "llm".to_string()
    }
}

/// Which transport a client came from — used by logs and by tests that must
/// not depend on downcasting a trait object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Ollama,
    Cloud,
    /// Claude 原生协议（spec-m2.1）
    Anthropic,
}

/// A single chat message. `role` is "system" | "user" | "assistant".
#[derive(Debug, Clone, serde::Serialize)]
pub struct Msg {
    role: String,
    content: String,
}

impl Msg {
    pub fn system(content: impl Into<String>) -> Self {
        Msg { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Msg { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Msg { role: "assistant".into(), content: content.into() }
    }

    pub fn role(&self) -> &str {
        &self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

// -------------------------------------------------------------- factory

/// Override layer → concrete client. Pure (takes the config + entries it
/// needs) so branches can be tested without touching the global cell.
pub(crate) fn choose(
    cfg: Option<&crate::llm_admin::LlmConfig>,
    entries: &[crate::llm_cloud::CloudEntry],
) -> (Arc<dyn ChatClient>, ClientKind) {
    if let Some(c) = cfg {
        if c.provider == crate::llm_admin::Provider::Cloud {
            if let Some(id) = c.active_id.strip_prefix(crate::llm_cloud::CLOUD_PREFIX) {
                if let Some(e) = entries.iter().find(|e| e.id == id) {
                    // Protocol follows the entry kind: Claude keeps its own
                    // headers/body shape, everything else speaks OpenAI.
                    return match e.kind {
                        crate::llm_cloud::EntryKind::Claude => (
                            Arc::new(AnthropicClient::new(
                                e.base_url.clone(),
                                e.model.clone(),
                                e.key.clone(),
                            )),
                            ClientKind::Anthropic,
                        ),
                        _ => (
                            Arc::new(CloudClient::new(
                                e.base_url.clone(),
                                e.model.clone(),
                                e.key.clone(),
                            )),
                            ClientKind::Cloud,
                        ),
                    };
                }
                // A deleted-but-still-active entry must never lock the app
                // out of speaking: fall back to the local leg and say so.
                log::warn!(
                    "active cloud entry `{id}` no longer exists — falling back to the local model"
                );
            }
        }
    }
    (Arc::new(OllamaClient::from_env()), ClientKind::Ollama)
}

/// The one factory the model legs use (spec-m2 §1.1). Read per call, so a
/// provider/model switch takes effect on the next message.
pub fn current_client() -> Arc<dyn ChatClient> {
    let cfg = crate::llm_admin::current();
    choose(cfg.as_ref(), &crate::llm_cloud::entries()).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_admin::{LlmConfig, Provider};
    use crate::llm_cloud::{CloudEntry, EntryKind};

    fn cloud_cfg(active: &str) -> LlmConfig {
        LlmConfig {
            provider: Provider::Cloud,
            local_model: DEFAULT_MODEL.into(),
            active_id: active.into(),
        }
    }

    fn entry(kind: EntryKind, url: &str) -> CloudEntry {
        CloudEntry {
            id: "c1".into(),
            name: "x".into(),
            kind,
            base_url: url.into(),
            model: "m".into(),
            key: "k".into(),
        }
    }

    #[test]
    fn roles_serialize_lowercase() {
        // 三种协议都按 "system"/"user"/"assistant" 认角色，不能变。
        let msgs = vec![Msg::system("s"), Msg::user("u"), Msg::assistant("a")];
        let json = serde_json::to_value(&msgs).unwrap();
        assert_eq!(json[0]["role"], serde_json::json!("system"));
        assert_eq!(json[1]["role"], serde_json::json!("user"));
        assert_eq!(json[2]["role"], serde_json::json!("assistant"));
    }

    // ---------- 工厂（4，spec-m2 §5.5 + spec-m2.1 §2）

    #[test]
    fn choose_falls_back_to_ollama_without_config() {
        let (_c, kind) = choose(None, &[]);
        assert_eq!(kind, ClientKind::Ollama);
    }

    #[test]
    fn choose_builds_cloud_client_for_active_entry() {
        let entries = vec![entry(EntryKind::Custom, "https://api.deepseek.com/v1")];
        let (_c, kind) = choose(Some(&cloud_cfg("cloud:c1")), &entries);
        assert_eq!(kind, ClientKind::Cloud);
    }

    #[test]
    fn choose_falls_back_when_active_entry_is_gone() {
        let entries = vec![entry(EntryKind::Custom, "https://x/v1")];
        let (_c, kind) = choose(Some(&cloud_cfg("cloud:deleted")), &entries);
        assert_eq!(kind, ClientKind::Ollama, "条目缺失必须回落本地而不是 panic");
    }

    #[test]
    fn choose_builds_anthropic_client_for_claude_entry() {
        // spec-m2.1：kind 决定协议，不是 base URL 猜的。
        let entries = vec![entry(EntryKind::Claude, ANTHROPIC_BASE_URL)];
        let (client, kind) = choose(Some(&cloud_cfg("cloud:c1")), &entries);
        assert_eq!(kind, ClientKind::Anthropic);
        assert!(client.label().starts_with("claude:"), "label 要能区分协议");
    }
}
