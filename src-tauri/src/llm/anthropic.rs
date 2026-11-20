//! Claude 原生协议（spec-m2.1）：`POST {base}/v1/messages`。
//!
//! 与 OpenAI 兼容端的差别只有三处，但三处都致命，所以单独一个 transport：
//! 1. 认证 header 是 `x-api-key`（不是 `Authorization: Bearer`），且必须带
//!    `anthropic-version`。
//! 2. system prompt 是**顶层字段**，不能留在 messages 里——bonsai 的人格卡
//!    恰好走这条路，拆错就等于人格丢失。
//! 3. 流式事件的形状不同：`content_block_delta` → `delta.text`。

use std::time::Duration;

use serde::Deserialize;

use super::{ChatClient, LlmError, Msg, ANTHROPIC_VERSION, CLOUD_MAX_TOKENS};

#[derive(Debug, Clone)]
pub struct AnthropicClient {
    base_url: String,
    model: String,
    api_key: String,
}

impl AnthropicClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        AnthropicClient { base_url: base_url.into(), model: model.into(), api_key: api_key.into() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// 默认端点是 `https://api.anthropic.com`（不含 `/v1`），但用户手填时
    /// 常常照 OpenAI 的习惯带上 `/v1`——两种写法都归一到一个 URL。
    fn endpoint(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        format!("{base}/v1/messages")
    }

    /// 一枪测试：1 token 请求，验证 base URL + key + 模型名三件可填错的事。
    pub fn ping(&self, timeout: u64) -> Result<(), LlmError> {
        let body = anthropic_body(&self.model, &[Msg::user("ping")], 0.0, false, 1);
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
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("Content-Type", "application/json")
            .set(
                "Accept",
                if stream { "text/event-stream" } else { "application/json" },
            )
            .send_string(&body.to_string())
            .map_err(|e| match e {
                // 非 2xx 的 body 才是「模型名写错 / key 无效」的唯一线索。
                ureq::Error::Status(code, resp) => {
                    let raw = resp.into_string().unwrap_or_default();
                    log::warn!(
                        "claude llm {} failed {code}: {}",
                        self.endpoint(),
                        super::cloud::error_message(&raw)
                    );
                    LlmError::Status(code)
                }
                other => LlmError::Transport(other.to_string()),
            })
    }
}

/// `/v1/messages` 请求体。纯函数——system 的拆分规则值得单测。
pub fn anthropic_body(
    model: &str,
    messages: &[Msg],
    temperature: f64,
    stream: bool,
    max_tokens: u32,
) -> serde_json::Value {
    let system: Vec<&str> = messages
        .iter()
        .filter(|m| m.role() == "system")
        .map(|m| m.content())
        .collect();
    let turns: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role() != "system")
        .map(|m| serde_json::json!({ "role": m.role(), "content": m.content() }))
        .collect();

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": turns,
        "temperature": temperature,
        "stream": stream,
    });
    // 多条 system 合成一段：Anthropic 只收一个字符串，人格卡 + 记忆摘要
    // 恰好是两条，用空行保住段落结构。
    if !system.is_empty() {
        body["system"] = serde_json::json!(system.join("\n\n"));
    }
    body
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<Block>,
}

#[derive(Debug, Deserialize)]
struct Block {
    #[serde(default)]
    text: String,
}

impl ChatClient for AnthropicClient {
    fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError> {
        let body = anthropic_body(&self.model, messages, temperature, false, CLOUD_MAX_TOKENS);
        let resp = self.post(&body, timeout, false)?;
        let parsed: AnthropicResponse =
            resp.into_json().map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(parsed.content.into_iter().map(|b| b.text).collect::<Vec<_>>().join(""))
    }

    fn chat_stream(
        &self,
        messages: &[Msg],
        temperature: f64,
        timeout: u64,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, LlmError> {
        let body = anthropic_body(&self.model, messages, temperature, true, CLOUD_MAX_TOKENS);
        let resp = self.post(&body, timeout, true)?;
        let reader = std::io::BufReader::new(resp.into_reader());
        use std::io::BufRead;
        let mut full = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| LlmError::Transport(e.to_string()))?;
            match parse_anthropic_sse_line(&line) {
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

    /// 与 `CloudClient` 同策略：付费端点没有免费探活，不额外烧一次调用。
    fn is_up(&self) -> bool {
        true
    }

    fn label(&self) -> String {
        format!("claude:{}", self.model)
    }
}

/// 解析一行 SSE。
///
/// `event:` 行直接忽略——正文的 `type` 字段与它等价，只认 `data:` 省一份状态。
/// `Ok(None)` = ping / message_start / content_block_* / message_delta /
/// message_stop（都没有正文）；`Err` = 服务端中途报错（Anthropic 用
/// `{"type":"error","error":{...}}` 表示，而不是 HTTP 状态码）。
pub fn parse_anthropic_sse_line(line: &str) -> Result<Option<String>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(None); // 保活注释
    }
    let Some(payload) = line.strip_prefix("data:") else {
        return Ok(None); // `event:` / `id:` / `retry:`
    };
    let payload = payload.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(None);
    }

    #[derive(Deserialize)]
    struct Event {
        #[serde(default, rename = "type")]
        kind: String,
        #[serde(default)]
        delta: Option<Delta>,
        #[serde(default)]
        error: Option<ErrObj>,
    }
    #[derive(Deserialize)]
    struct Delta {
        #[serde(default)]
        text: String,
    }
    #[derive(Deserialize)]
    struct ErrObj {
        #[serde(default)]
        message: String,
    }

    let ev: Event = serde_json::from_str(payload).map_err(|e| format!("bad sse line: {e}"))?;
    match ev.kind.as_str() {
        "content_block_delta" => Ok(ev
            .delta
            .map(|d| d.text)
            .filter(|t| !t.is_empty())),
        "error" => Err(ev
            .error
            .map(|e| e.message)
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "claude error".into())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- SSE 解析（4，spec-m2.1 §2）

    #[test]
    fn delta_event_carries_text() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#;
        assert_eq!(parse_anthropic_sse_line(line).unwrap(), Some("你好".to_string()));
    }

    #[test]
    fn message_stop_and_bookkeeping_are_silent() {
        assert_eq!(parse_anthropic_sse_line(r#"data: {"type":"message_stop"}"#).unwrap(), None);
        assert_eq!(
            parse_anthropic_sse_line(r#"data: {"type":"message_start","message":{"id":"m1"}}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_anthropic_sse_line(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_anthropic_sse_line(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#).unwrap(),
            None
        );
        assert_eq!(parse_anthropic_sse_line("event: content_block_delta").unwrap(), None);
        assert_eq!(parse_anthropic_sse_line(": keepalive").unwrap(), None);
    }

    #[test]
    fn ping_is_ignored() {
        assert_eq!(parse_anthropic_sse_line(r#"data: {"type":"ping"}"#).unwrap(), None);
        assert_eq!(
            parse_anthropic_sse_line(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}"#).unwrap(),
            None,
            "空 delta 不该产生一次播报"
        );
    }

    #[test]
    fn error_event_surfaces_the_reason() {
        let line = r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(parse_anthropic_sse_line(line).unwrap_err(), "Overloaded".to_string());
        assert_eq!(
            parse_anthropic_sse_line(r#"data: {"type":"error","error":{}}"#).unwrap_err(),
            "claude error".to_string()
        );
        assert!(parse_anthropic_sse_line("data: not-json").is_err(), "垃圾行必须报错不能静默");
    }

    // ---------- 请求体（2）

    #[test]
    fn body_lifts_system_prompt_out_of_messages() {
        let msgs = vec![
            Msg::system("你是阿盆"),
            Msg::user("在吗"),
            Msg::assistant("在"),
            Msg::user("今天好累"),
        ];
        let body = anthropic_body("claude-3-5-sonnet-latest", &msgs, 0.7, true, CLOUD_MAX_TOKENS);

        assert_eq!(body["system"], serde_json::json!("你是阿盆"), "system 必须是顶层字段");
        let turns = body["messages"].as_array().unwrap();
        assert_eq!(turns.len(), 3, "messages 里不得残留 system");
        assert_eq!(turns[0]["role"], serde_json::json!("user"));
        assert_eq!(turns[1]["role"], serde_json::json!("assistant"));
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(body["temperature"], serde_json::json!(0.7));
        assert_eq!(body["max_tokens"], serde_json::json!(4096));
    }

    #[test]
    fn body_without_system_omits_the_field_and_merges_multi_system() {
        let plain = anthropic_body("m", &[Msg::user("hi")], 0.0, false, 1);
        assert!(plain.get("system").is_none(), "没有 system 就不该带这个字段");

        // 人格卡 + 记忆摘要两条 system → 合成一段，用空行分段。
        let merged = anthropic_body(
            "m",
            &[Msg::system("人格"), Msg::system("记忆"), Msg::user("hi")],
            0.3,
            false,
            16,
        );
        assert_eq!(merged["system"], serde_json::json!("人格\n\n记忆"));
        assert_eq!(merged["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn endpoint_accepts_both_base_flavours() {
        // 默认常量不带 /v1；用户照 OpenAI 习惯填 /v1 也不能拼成 /v1/v1。
        assert_eq!(
            AnthropicClient::new(super::super::ANTHROPIC_BASE_URL, "m", "k").endpoint(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            AnthropicClient::new("https://api.anthropic.com/v1/", "m", "k").endpoint(),
            "https://api.anthropic.com/v1/messages"
        );
    }
}
