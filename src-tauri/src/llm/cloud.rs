//! OpenAI-compatible transport (`/chat/completions`).
//!
//! DeepSeek / Moonshot / GLM / Qwen compatible / OpenRouter / OpenAI itself
//! all speak this dialect. One entry = one client; the api key is held in
//! memory only for the duration of the client, and comes from the local
//! settings table (plaintext — that trade-off is spelled out in the opt-in
//! modal, spec-m2 §4).

use std::time::Duration;

use serde::Deserialize;

use super::{ChatClient, LlmError, Msg, CLOUD_MAX_TOKENS};

#[derive(Debug, Clone)]
pub struct CloudClient {
    base_url: String,
    model: String,
    api_key: String,
}

impl CloudClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        CloudClient { base_url: base_url.into(), model: model.into(), api_key: api_key.into() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Tolerates a trailing slash: users paste `https://x/v1` and `.../v1/`.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim().trim_end_matches('/'))
    }

    /// One-token probe used by "测试连接" and by the switch path. Success means
    /// the endpoint accepts our base URL, key and model name — the three
    /// things a user can typo.
    pub fn ping(&self, timeout: u64) -> Result<(), LlmError> {
        let body = chat_body(&self.model, &[Msg::user("ping")], 0.0, false, 1);
        self.post(&body, timeout, false).map(|_| ())
    }

    fn post(
        &self,
        body: &serde_json::Value,
        timeout: u64,
        stream: bool,
    ) -> Result<ureq::Response, LlmError> {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(timeout))
            .build()
            .post(&self.endpoint())
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .set(
                "Accept",
                if stream { "text/event-stream" } else { "application/json" },
            )
            .send_string(&body.to_string())
            .map_err(|e| match e {
                // Non-2xx: the body carries the provider's reason ("model not
                // found", "invalid api key") — that reason is the only way to
                // debug a bad entry, so it goes to the log.
                ureq::Error::Status(code, resp) => {
                    let raw = resp.into_string().unwrap_or_default();
                    log::warn!("cloud llm {} failed {code}: {}", self.endpoint(), error_message(&raw));
                    LlmError::Status(code)
                }
                other => LlmError::Transport(other.to_string()),
            })
    }
}

/// Pull `error.message` out of a provider error body, else the raw body
/// (truncated — a 500 can return an HTML page).
pub fn error_message(body: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let msg = parsed
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string());
    msg.trim().chars().take(200).collect()
}

/// Request body for `/chat/completions`. Pure so the shape has tests.
pub fn chat_body(
    model: &str,
    messages: &[Msg],
    temperature: f64,
    stream: bool,
    max_tokens: u32,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": stream,
        "temperature": temperature,
        "max_tokens": max_tokens,
    })
}

/// 工具轮请求体（plan-task-card v0.2 §1.1）。与 `chat_body` 分开：
/// 带 `tools` + `tool_choice:"auto"`，恒非流式（工具轮的响应是
/// tool_calls JSON，流式展示会闪 JSON；最终作答轮 v1 也非流式）。
/// 思考模式不支持 `tool_choice` 的 required/指定工具（协议 400），且
/// DeepSeek 扩展字段（thinking/title/safety_check）一概不碰——只发
/// 标准 OpenAI 兼容三件套。
///
/// `tools` 为空（强制收尾轮）时整个省略 `tools`/`tool_choice` 键——
/// 协议语义「没有 tool 时默认 tool_choice=none」，模型只能散文作答；
/// 不发空数组（部分实现会拒绝）。
pub fn tool_chat_body(
    model: &str,
    messages: &[crate::llm::ToolMsg],
    tools: &[crate::llm::ToolSpec],
    temperature: f64,
    max_tokens: u32,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": temperature,
        "max_tokens": max_tokens,
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
        body["tool_choice"] = serde_json::json!("auto");
    }
    body
}

/// 工具轮响应解析（纯函数，形状有测试）。`finish_reason` 取
/// `choices[0].finish_reason`；`tool_calls` 的 `arguments` 按协议是
/// JSON 字符串，解析失败时保留 `Value::String(原文)` 交给本地校验拒绝。
pub fn parse_tool_response(body: &serde_json::Value) -> Result<crate::llm::ToolReply, LlmError> {
    let choice = body
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| LlmError::Decode("missing choices".into()))?;
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop")
        .to_string();
    let message = choice.get("message");
    let content = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
        .filter(|c| !c.trim().is_empty());
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.and_then(|m| m.get("tool_calls")).and_then(|t| t.as_array()) {
        for call in calls {
            let id = call.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let raw_args = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let arguments = serde_json::from_str(raw_args)
                .unwrap_or(serde_json::Value::String(raw_args.to_string()));
            tool_calls.push(crate::llm::ToolCallReq { id, name, arguments });
        }
    }
    Ok(crate::llm::ToolReply { content, tool_calls, finish_reason })
}

#[derive(Debug, Deserialize)]
struct CloudResponse {
    #[serde(default)]
    choices: Vec<CloudChoice>,
}

#[derive(Debug, Deserialize)]
struct CloudChoice {
    #[serde(default)]
    message: Option<CloudMessage>,
}

#[derive(Debug, Deserialize)]
struct CloudMessage {
    #[serde(default)]
    content: Option<String>,
}

impl ChatClient for CloudClient {
    fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError> {
        let body = chat_body(&self.model, messages, temperature, false, CLOUD_MAX_TOKENS);
        let resp = self.post(&body, timeout, false)?;
        let parsed: CloudResponse = resp.into_json().map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .and_then(|m| m.content)
            .unwrap_or_default())
    }

    fn chat_stream(
        &self,
        messages: &[Msg],
        temperature: f64,
        timeout: u64,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, LlmError> {
        let body = chat_body(&self.model, messages, temperature, true, CLOUD_MAX_TOKENS);
        let resp = self.post(&body, timeout, true)?;
        let reader = std::io::BufReader::new(resp.into_reader());
        use std::io::BufRead;
        let mut full = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| LlmError::Transport(e.to_string()))?;
            match parse_sse_line(&line) {
                Ok(Some(delta)) => {
                    full.push_str(&delta);
                    on_delta(&delta);
                }
                Ok(None) => {}
                Err(reason) => return Err(LlmError::Decode(reason)),
            }
        }
        Ok(full)
    }

    /// No free liveness probe exists for a paid endpoint, and a probe per
    /// call would double the bill. Optimistic: a dead endpoint surfaces as a
    /// `chat` error, which every caller already degrades on (empty reply →
    /// honest fallback line, extraction → RuleExtractor).
    fn is_up(&self) -> bool {
        true
    }

    fn label(&self) -> String {
        format!("cloud:{}", self.model)
    }

    /// 工具轮（plan-task-card v0.2 §4）：非流式 POST + 响应解析。
    fn chat_with_tools(
        &self,
        messages: &[crate::llm::ToolMsg],
        tools: &[crate::llm::ToolSpec],
        temperature: f64,
        timeout: u64,
    ) -> Result<crate::llm::ToolReply, LlmError> {
        let body = tool_chat_body(&self.model, messages, tools, temperature, CLOUD_MAX_TOKENS);
        let resp = self.post(&body, timeout, false)?;
        let parsed: serde_json::Value =
            resp.into_json().map_err(|e| LlmError::Decode(e.to_string()))?;
        parse_tool_response(&parsed)
    }
}

/// Parse one SSE line of `/chat/completions` streaming.
/// `Ok(None)` = keepalive / comment / `[DONE]` / delta without content.
/// Mirrors `parse_stream_line` (same protocol-handling test style); the error
/// arm exists because providers report failures mid-stream as an `error`
/// object rather than an HTTP status.
pub fn parse_sse_line(line: &str) -> Result<Option<String>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(None); // keepalive comment (`: ping`)
    }
    let Some(payload) = line.strip_prefix("data:") else {
        return Ok(None); // `event:` / `id:` / `retry:` — not content
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return Ok(None);
    }
    #[derive(Deserialize)]
    struct Chunk {
        #[serde(default)]
        choices: Vec<Choice>,
        #[serde(default)]
        error: Option<ErrObj>,
    }
    #[derive(Deserialize)]
    struct Choice {
        #[serde(default)]
        delta: Option<Delta>,
    }
    #[derive(Deserialize)]
    struct Delta {
        #[serde(default)]
        content: Option<String>,
    }
    #[derive(Deserialize)]
    struct ErrObj {
        #[serde(default)]
        message: String,
    }
    let chunk: Chunk = serde_json::from_str(payload).map_err(|e| format!("bad sse line: {e}"))?;
    if let Some(err) = chunk.error {
        return Err(if err.message.is_empty() { "cloud error".into() } else { err.message });
    }
    Ok(chunk
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta)
        .and_then(|d| d.content)
        .filter(|c| !c.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- SSE（6，spec-m2 §5.1）

    #[test]
    fn sse_line_parses_content_delta() {
        let line = r#"data: {"choices":[{"index":0,"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_line(line).unwrap(), Some("你好".to_string()));
    }

    #[test]
    fn sse_line_stops_at_done() {
        assert_eq!(parse_sse_line("data: [DONE]").unwrap(), None);
    }

    #[test]
    fn sse_line_ignores_blank_and_comment_lines() {
        assert_eq!(parse_sse_line("").unwrap(), None);
        assert_eq!(parse_sse_line("   ").unwrap(), None);
        assert_eq!(parse_sse_line(": ping").unwrap(), None);
        assert_eq!(parse_sse_line("event: message").unwrap(), None);
    }

    #[test]
    fn sse_line_without_choices_is_none() {
        // Role-only first chunk, and a usage-only last chunk: no text either way.
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap(), None);
        assert_eq!(parse_sse_line(r#"data: {"choices":[]}"#).unwrap(), None);
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{"content":""}}]}"#).unwrap(), None);
    }

    #[test]
    fn sse_line_surfaces_error_json() {
        let line = r#"data: {"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert_eq!(parse_sse_line(line).unwrap_err(), "model not found".to_string());
        assert_eq!(parse_sse_line(r#"data: {"error":{}}"#).unwrap_err(), "cloud error".to_string());
    }

    #[test]
    fn sse_line_garbage_is_an_error_not_silence() {
        // A non-JSON payload means the stream is not what we think it is:
        // surface it instead of silently returning a truncated reply.
        assert!(parse_sse_line("data: not-json").is_err());
    }

    // ---------- 请求体（3）

    #[test]
    fn cloud_body_carries_model_messages_and_flags() {
        let msgs = vec![Msg::system("s"), Msg::user("u")];
        let body = chat_body("deepseek-chat", &msgs, 0.6, true, CLOUD_MAX_TOKENS);
        assert_eq!(body["model"], serde_json::json!("deepseek-chat"));
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(body["temperature"], serde_json::json!(0.6));
        assert_eq!(body["max_tokens"], serde_json::json!(4096));
        assert_eq!(body["messages"][0]["role"], serde_json::json!("system"));
        assert_eq!(body["messages"][1]["content"], serde_json::json!("u"));
        // One-token probe reuses the same shape with a smaller cap.
        let probe = chat_body("m", &msgs, 0.0, false, 1);
        assert_eq!(probe["max_tokens"], serde_json::json!(1));
        assert_eq!(probe["stream"], serde_json::json!(false));
    }

    #[test]
    fn cloud_endpoint_normalises_slashes() {
        assert_eq!(
            CloudClient::new("https://api.deepseek.com/v1", "m", "k").endpoint(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            CloudClient::new("https://api.openai.com/v1/", "m", "k").endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(CloudClient::new("  https://x/v1  ", "m", "k").endpoint(), "https://x/v1/chat/completions");
    }

    #[test]
    fn cloud_error_message_prefers_provider_reason() {
        assert_eq!(
            error_message(r#"{"error":{"message":"invalid api key"}}"#),
            "invalid api key"
        );
        assert_eq!(error_message("<html>502</html>"), "<html>502</html>");
        assert!(error_message(&"x".repeat(500)).chars().count() <= 200);
    }

    // ---------- 工具轮（plan-task-card v0.2 §1.1，形状对齐官方文档样例）

    #[test]
    fn tool_body_carries_tools_and_choice() {
        use crate::llm::{ToolMsg, ToolSpec};
        let msgs = vec![ToolMsg::system("s"), ToolMsg::user("杭州天气？")];
        let tools = vec![ToolSpec {
            name: "weather".into(),
            description: "查天气".into(),
            parameters: Some(serde_json::json!({"type":"object"})),
        }];
        let body = tool_chat_body("deepseek-flash", &msgs, &tools, 0.7, CLOUD_MAX_TOKENS);
        assert_eq!(body["model"], serde_json::json!("deepseek-flash"));
        assert_eq!(body["tool_choice"], serde_json::json!("auto"));
        assert_eq!(body["stream"], serde_json::json!(false), "工具轮恒非流式");
        assert_eq!(body["tools"][0]["type"], serde_json::json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], serde_json::json!("weather"));
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], serde_json::json!("object"));
        // 不携带 DeepSeek 扩展字段（thinking/title/safety_check）
        assert!(body.get("thinking").is_none());
        assert!(body.get("title").is_none());
        assert!(body.get("safety_check").is_none());
        // 无参数工具：parameters 省略（协议规定省略即空参数表）
        let no_args = vec![ToolSpec { name: "t".into(), description: "d".into(), parameters: None }];
        let b2 = tool_chat_body("m", &msgs, &no_args, 0.7, CLOUD_MAX_TOKENS);
        assert!(b2["tools"][0]["function"].get("parameters").is_none());
        // 空 tools（强制收尾轮）→ 整个省略 tools/tool_choice 键
        let b3 = tool_chat_body("m", &msgs, &[], 0.7, CLOUD_MAX_TOKENS);
        assert!(b3.get("tools").is_none());
        assert!(b3.get("tool_choice").is_none());
    }

    #[test]
    fn tool_response_parses_calls_and_stop() {
        // finish_reason=tool_calls（官方文档样例形状）
        let body: serde_json::Value = serde_json::from_str(
            r#"{"choices":[{"finish_reason":"tool_calls","message":{
                "role":"assistant","content":null,
                "tool_calls":[{"id":"call_abc","type":"function",
                    "function":{"name":"get_weather","arguments":"{\"location\":\"杭州\"}"}}]}}],
              "created":1,"model":"deepseek-flash","object":"chat.completion"}"#,
        )
        .unwrap();
        let reply = parse_tool_response(&body).unwrap();
        assert_eq!(reply.finish_reason, "tool_calls");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "call_abc");
        assert_eq!(reply.tool_calls[0].name, "get_weather");
        assert_eq!(reply.tool_calls[0].arguments["location"], serde_json::json!("杭州"));
        assert!(reply.content.is_none());

        // finish_reason=stop → 正文
        let stop: serde_json::Value = serde_json::from_str(
            r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"多云 18 度。"}}]}"#,
        )
        .unwrap();
        let reply = parse_tool_response(&stop).unwrap();
        assert_eq!(reply.finish_reason, "stop");
        assert!(reply.tool_calls.is_empty());
        assert_eq!(reply.content.as_deref(), Some("多云 18 度。"));
    }

    #[test]
    fn tool_response_keeps_invalid_argument_json_as_string() {
        // 协议明说「模型并不总是生成有效的 JSON」：解析失败的 arguments
        // 保留原文成 Value::String，交给 validate_args 产出 bad_args 回传。
        let body: serde_json::Value = serde_json::from_str(
            r#"{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant",
                "tool_calls":[{"id":"x","type":"function","function":{"name":"t","arguments":"不是json"}}]}}]}"#,
        )
        .unwrap();
        let reply = parse_tool_response(&body).unwrap();
        assert_eq!(reply.tool_calls[0].arguments, serde_json::Value::String("不是json".into()));
    }

    #[test]
    fn tool_response_garbage_is_an_error() {
        assert!(parse_tool_response(&serde_json::json!({"choices":[]})).is_err());
        assert!(parse_tool_response(&serde_json::json!({})).is_err());
    }
}
