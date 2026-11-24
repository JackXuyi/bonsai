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
pub mod schema;
pub mod serve;
pub mod watchdog;

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

// -------------------------------------------------------------- tool calling
//
// 工具调用分流（plan-task-card v0.2，2026-11-22 用户裁决）：本地模型
// **永远不做**工具调用——只有支持 function calling 的云端传输实现
// `chat_with_tools`；其余实现走 trait 默认 Err，`tool_unsupported` 由
// 类型系统强制，而非运行时约定。协议对齐 DeepSeek Chat Completion
// （api-docs.deepseek.com/zh-cn/guides/tool_calls）：
//   请求 tools[] = {type:"function", function:{name, description, parameters}}
//   响应 finish_reason="tool_calls" + message.tool_calls[]{id, function:{name, arguments}}
//   结果回传 {"role":"tool","tool_call_id":<id>,"content":<摘要>}

/// 一个可被模型调用的工具定义（由 `ToolDescriptor` 生成）。`parameters`
/// 是 JSON Schema；`None` = 无参数（协议规定：省略 parameters 即空参数表）。
/// 序列化成协议要求的 `{type:"function", function:{name, description, parameters}}`。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
}

impl serde::Serialize for ToolSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolSpec", 2)?;
        st.serialize_field("type", "function")?;
        st.serialize_field(
            "function",
            &ToolSpecFunc { name: &self.name, description: &self.description, parameters: &self.parameters },
        )?;
        st.end()
    }
}

struct ToolSpecFunc<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Option<serde_json::Value>,
}

impl serde::Serialize for ToolSpecFunc<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolSpecFunc", 3)?;
        st.serialize_field("name", self.name)?;
        st.serialize_field("description", self.description)?;
        if let Some(p) = self.parameters {
            st.serialize_field("parameters", p)?;
        }
        st.end()
    }
}

/// 助手消息携带的「我调用了这些工具」记录（回传时的形状）。
/// `arguments` 保持协议原样——JSON 字符串，不是解析后的 Value。
/// 序列化成协议要求的 `{id, type:"function", function:{name, arguments}}`。
#[derive(Debug, Clone)]
pub struct ToolCallOut {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

struct ToolCallFunc<'a> {
    name: &'a str,
    arguments: &'a str,
}

impl serde::Serialize for ToolCallOut {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolCallOut", 3)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("type", "function")?;
        st.serialize_field("function", &ToolCallFunc { name: &self.name, arguments: &self.arguments })?;
        st.end()
    }
}

impl<'a> serde::Serialize for ToolCallFunc<'a> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolCallFunc", 2)?;
        st.serialize_field("name", self.name)?;
        st.serialize_field("arguments", self.arguments)?;
        st.end()
    }
}

/// 工具循环的消息（`Msg` 的工具扩展）。role ∈ system|user|assistant|tool。
/// 刻意不复用 `Msg`：工具轮的消息形状不同（assistant 可带 tool_calls、
/// tool 消息带 tool_call_id），单独结构避免污染全部既有调用点。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolMsg {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ToolMsg {
    pub fn system(content: impl Into<String>) -> Self {
        ToolMsg { role: "system".into(), content: Some(content.into()), tool_calls: Vec::new(), tool_call_id: None }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ToolMsg { role: "user".into(), content: Some(content.into()), tool_calls: Vec::new(), tool_call_id: None }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ToolMsg { role: "assistant".into(), content: Some(content.into()), tool_calls: Vec::new(), tool_call_id: None }
    }
    /// assistant 的工具调用声明（Chat Completion 协议：assistant 消息的
    /// content 可为空，tool_calls 必须原样回传）。
    pub fn assistant_tool_calls(calls: Vec<ToolCallOut>) -> Self {
        ToolMsg { role: "assistant".into(), content: None, tool_calls: calls, tool_call_id: None }
    }
    /// 工具执行结果。`tool_call_id` 必须对应上一次 assistant 消息里的 id。
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ToolMsg { role: "tool".into(), content: Some(content.into()), tool_calls: Vec::new(), tool_call_id: Some(call_id.into()) }
    }
}

/// 模型要求的一次工具调用。`arguments` 已从协议的 JSON 字符串解析；
/// 解析失败时是 `Value::String(原文)`——交给 `validate_args` 产出
/// `bad_args:not-an-object`，错误以 tool 消息回传（协议明说模型可能
/// 臆造参数，本地校验是硬兜底）。
#[derive(Debug, Clone)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 一次工具轮的响应。`tool_calls` 非空 ⇒ 继续循环；空且 `content` 有值
/// ⇒ `finish_reason="stop"`，content 即最终回复。
#[derive(Debug, Clone)]
pub struct ToolReply {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCallReq>,
    pub finish_reason: String,
}

/// 可 Clone 是为了测试桩能按脚本重放同一组错误（plan-retry §5.1）。
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    #[error("http transport error: {0}")]
    Transport(String),
    /// 第二个字段是响应体摘要（`error_message`，≤200 字）：401 的
    /// 「invalid api key」与 502 的「上游崩了」必须能区分（plan-retry §1）。
    #[error("server returned {0}: {1}")]
    Status(u16, String),
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

    /// 工具调用轮（plan-task-card v0.2 §3）。默认 `tool_unsupported`：
    /// 本地模型不做工具调用，`DialogueBackend::tool_client` 只在云端
    /// 传输上返回 Some——「本地不执行工具调用」由类型形状强制。
    fn chat_with_tools(
        &self,
        _messages: &[ToolMsg],
        _tools: &[ToolSpec],
        _temperature: f64,
        _timeout: u64,
    ) -> Result<ToolReply, LlmError> {
        Err(LlmError::Decode("tool_unsupported".into()))
    }

    /// 结构化输出（plan-structured-routing §3.1）：传输层用原生机制约束
    /// 输出为合法 JSON。返回原始字符串，解析与语义校验留给调用点。
    /// 默认不支持（Anthropic 等无原生字段的腿走兜底，不做协议 hack）。
    fn chat_structured(
        &self,
        _messages: &[Msg],
        _schema: &serde_json::Value,
        _temperature: f64,
        _timeout: u64,
    ) -> Result<String, LlmError> {
        Err(LlmError::Decode("structured_unsupported".into()))
    }
}

/// 两条腿（plan-structured-routing §3.5）：对话跟随主配置；后台任务可钉本地。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLeg {
    Chat,
    Background,
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
/// 等价于 `client_for(TaskLeg::Chat)`。
pub fn current_client() -> Arc<dyn ChatClient> {
    current_client_with_kind().0
}

/// Same factory, plus the transport kind — the dialogue layer's tool path
/// needs it (只有云端传输走 function calling；本地/Anthropic 走本地路径)。
pub fn current_client_with_kind() -> (Arc<dyn ChatClient>, ClientKind) {
    let cfg = crate::llm_admin::current();
    choose(cfg.as_ref(), &crate::llm_cloud::entries())
}

/// 按任务腿取客户端（plan-structured-routing §3.5）。
/// - Chat：始终跟随主配置（= `current_client`）。
/// - Background：Manual（默认）与 Chat 同源；Auto 恒走本地小模型。
pub fn client_for(leg: TaskLeg) -> Arc<dyn ChatClient> {
    match leg {
        TaskLeg::Chat => current_client(),
        TaskLeg::Background => background_client(),
    }
}

fn background_client() -> Arc<dyn ChatClient> {
    if crate::llm_admin::routing_auto() {
        let client = Arc::new(OllamaClient::from_env()) as Arc<dyn ChatClient>;
        log::info!("background leg -> {}", client.label());
        client
    } else {
        let client = current_client();
        log::info!("background leg -> {} (follow config)", client.label());
        client
    }
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
            routing: crate::llm_admin::RouteMode::FollowConfig,
        }
    }

    #[test]
    fn default_chat_structured_is_unsupported() {
        struct Stub;
        impl ChatClient for Stub {
            fn chat(&self, _: &[Msg], _: f64, _: u64) -> Result<String, LlmError> {
                Ok(String::new())
            }
            fn chat_stream(
                &self,
                _: &[Msg],
                _: f64,
                _: u64,
                _: &mut dyn FnMut(&str),
            ) -> Result<String, LlmError> {
                Ok(String::new())
            }
            fn is_up(&self) -> bool {
                true
            }
        }
        let err = Stub
            .chat_structured(&[], &serde_json::json!({}), 0.2, 1)
            .unwrap_err();
        match err {
            LlmError::Decode(m) => assert_eq!(m, "structured_unsupported"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn background_leg_pins_local_when_auto() {
        use crate::llm_admin::{self, RouteMode};
        let _g = llm_admin::test_lock();
        llm_admin::clear_for_test();
        crate::llm_cloud::set_cache_for_test(vec![entry(
            EntryKind::Custom,
            "https://api.deepseek.com/v1",
        )]);

        let mut cfg = cloud_cfg("cloud:c1");
        cfg.routing = RouteMode::FollowConfig;
        llm_admin::apply(cfg.clone());
        // Manual：后台与 Chat 同源 → 云
        assert!(client_for(TaskLeg::Chat).label().starts_with("cloud:"));
        assert_eq!(
            client_for(TaskLeg::Chat).label(),
            client_for(TaskLeg::Background).label()
        );

        cfg.routing = RouteMode::AutoLocal;
        llm_admin::apply(cfg);
        assert!(
            client_for(TaskLeg::Chat).label().starts_with("cloud:"),
            "Chat 仍跟主配置"
        );
        assert!(
            client_for(TaskLeg::Background).label().starts_with("ollama:"),
            "Auto 后台钉本地"
        );

        llm_admin::clear_for_test();
        crate::llm_cloud::set_cache_for_test(Vec::new());
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

    // ---------- 工具协议（plan-task-card v0.2 §3）

    #[test]
    fn tool_msg_serializes_tool_round_shapes() {
        // user → assistant(tool_calls) → tool 回传：DeepSeek 协议三件套。
        let msgs = vec![
            ToolMsg::system("s"),
            ToolMsg::user("杭州天气？"),
            ToolMsg::assistant_tool_calls(vec![ToolCallOut {
                id: "call_1".into(),
                name: "weather".into(),
                arguments: r#"{"city":"杭州"}"#.into(),
            }]),
            ToolMsg::tool_result("call_1", "多云 18 度"),
        ];
        let json = serde_json::to_value(&msgs).unwrap();
        assert_eq!(json[0]["role"], serde_json::json!("system"));
        assert_eq!(json[1]["role"], serde_json::json!("user"));
        // assistant: content 为空时省略（协议允许 null content + tool_calls）
        assert!(json[2].get("content").is_none(), "empty content must be omitted");
        assert_eq!(json[2]["tool_calls"][0]["id"], serde_json::json!("call_1"));
        assert_eq!(json[2]["tool_calls"][0]["function"]["name"], serde_json::json!("weather"));
        assert_eq!(json[2]["tool_calls"][0]["function"]["arguments"], serde_json::json!(r#"{"city":"杭州"}"#));
        // tool 消息：tool_call_id 对应
        assert_eq!(json[3]["role"], serde_json::json!("tool"));
        assert_eq!(json[3]["tool_call_id"], serde_json::json!("call_1"));
        assert_eq!(json[3]["content"], serde_json::json!("多云 18 度"));
        assert!(json[3].get("tool_calls").is_none(), "tool 结果消息不带 tool_calls");
    }

    #[test]
    fn default_chat_with_tools_is_unsupported() {
        // 「本地模型不做工具调用」的强制点：任何未实现的传输（含测试桩）
        // 拿到 Err，不产生任何 HTTP 往返。
        struct NoTools;
        impl ChatClient for NoTools {
            fn chat(&self, _m: &[Msg], _t: f64, _to: u64) -> Result<String, LlmError> {
                Ok(String::new())
            }
            fn chat_stream(
                &self,
                _m: &[Msg],
                _t: f64,
                _to: u64,
                _d: &mut dyn FnMut(&str),
            ) -> Result<String, LlmError> {
                Ok(String::new())
            }
            fn is_up(&self) -> bool {
                true
            }
        }
        let err = NoTools.chat_with_tools(&[], &[], 0.7, 10).unwrap_err();
        assert!(err.to_string().contains("tool_unsupported"), "got: {err}");
    }
}
