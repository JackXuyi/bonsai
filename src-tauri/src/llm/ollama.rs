//! The bundled-sidecar transport (Ollama-compatible HTTP API).
//!
//! Split out of `llm.rs` (spec-m2.1) so each protocol owns a file; the trait
//! and the factory stay in `llm/mod.rs`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ChatClient, LlmError, Msg, DEFAULT_BASE_URL, DEFAULT_MODEL, PROBE_TIMEOUT_SECS};

/// 本地上下文窗口（plan-context D5，2026-11-23 裁决 8192）。
///
/// 以前这里填 0 = 交给 Ollama 默认值（通常 2048/4096）：超窗时 Ollama 丢的是
/// **开头**，也就是最该保住的人格宪法——静默截断比报错更危险。显式设定后
/// `context::window_for` 才能算出真实可用额度（usable = 8192 − 4096 预留）。
/// 2B 首字延迟若明显变差，可回落到 4096（只改这一个常量，全链路跟着变）。
pub const NUM_CTX: usize = 8192;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Msg],
    stream: bool,
    /// Qwen3-class models default to a visible reasoning pass that can burn
    /// the whole token budget before emitting any answer (observed with
    /// qwen3:4b: `content` came back empty, `done_reason: length`). Both legs
    /// want direct output — dialogue for latency, extraction for clean JSON.
    think: bool,
    options: Options,
}

#[derive(Debug, Serialize)]
struct Options {
    temperature: f64,
    #[serde(skip_serializing_if = "is_zero")]
    num_ctx: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: RespMessage,
}

#[derive(Debug, Deserialize)]
struct RespMessage {
    content: String,
}

impl OllamaClient {
    /// Priority (spec-m1 §1.1): UI override (llm_admin) > env > defaults.
    /// Read on every call — a model switch takes effect without a restart,
    /// and the four call sites never need to know about the settings.
    pub fn from_env() -> Self {
        OllamaClient {
            base_url: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
            model: crate::llm_admin::override_model()
                .or_else(|| std::env::var("BONSAI_LLM_MODEL").ok())
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }

    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        OllamaClient { base_url: base_url.into(), model: model.into() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Cheap liveness probe: the server answers `/api/version` without
    /// loading any model.
    pub fn is_up(&self) -> bool {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
            .build()
            .get(&format!("{}/api/version", self.base_url))
            .call()
            .is_ok()
    }

    /// One chat completion. `messages` must already carry the system prompt
    /// (persona card for dialogue, extraction instructions for memory).
    pub fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError> {
        let req = self.request(messages, temperature, false);
        let resp = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(timeout))
            .build()
            .post(&format!("{}/api/chat", self.base_url))
            .send_json(&req)
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => LlmError::Status(
                    code,
                    resp.into_string()
                        .map(|b| super::cloud::error_message(&b))
                        .unwrap_or_default(),
                ),
                other => LlmError::Transport(other.to_string()),
            })?;
        let body: ChatResponse = resp
            .into_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(body.message.content)
    }

    fn request<'a>(&'a self, messages: &'a [Msg], temperature: f64, stream: bool) -> ChatRequest<'a> {
        ChatRequest {
            model: &self.model,
            messages,
            stream,
            think: false,
            options: Options { temperature, num_ctx: NUM_CTX },
        }
    }

    /// Streaming chat: same request with `stream: true`, feeding every
    /// content chunk to `on_delta` as it arrives, returning the full reply.
    /// The transport is NDJSON (one JSON object per line).
    pub fn chat_stream(
        &self,
        messages: &[Msg],
        temperature: f64,
        timeout: u64,
        mut on_delta: impl FnMut(&str),
    ) -> Result<String, LlmError> {
        let req = self.request(messages, temperature, true);
        let resp = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(timeout))
            .build()
            .post(&format!("{}/api/chat", self.base_url))
            .send_json(&req)
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => LlmError::Status(
                    code,
                    resp.into_string()
                        .map(|b| super::cloud::error_message(&b))
                        .unwrap_or_default(),
                ),
                other => LlmError::Transport(other.to_string()),
            })?;
        let reader = std::io::BufReader::new(resp.into_reader());
        use std::io::BufRead;
        let mut full = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| LlmError::Transport(e.to_string()))?;
            match parse_stream_line(&line) {
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
}

impl ChatClient for OllamaClient {
    fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError> {
        let r = OllamaClient::chat(self, messages, temperature, timeout);
        // plan-retry §5.6：本地传输的每次请求都喂给看门狗（成功也报，
        // 用于清「连续 down」计数）。未 init（单测）时是 no-op。
        super::watchdog::record_request(r.is_ok());
        r
    }

    fn chat_stream(
        &self,
        messages: &[Msg],
        temperature: f64,
        timeout: u64,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, LlmError> {
        let r = OllamaClient::chat_stream(self, messages, temperature, timeout, |d: &str| on_delta(d));
        super::watchdog::record_request(r.is_ok());
        r
    }

    fn is_up(&self) -> bool {
        OllamaClient::is_up(self)
    }

    fn label(&self) -> String {
        format!("ollama:{}", self.model)
    }
}

/// Parse one NDJSON line of the streaming chat API into the content delta.
/// `Ok(None)` = keepalive / final bookkeeping line with no content.
/// Pure function so the protocol handling has unit tests.
pub fn parse_stream_line(line: &str) -> Result<Option<String>, String> {
    if line.trim().is_empty() {
        return Ok(None); // keepalive / blank line
    }
    #[derive(Deserialize)]
    struct StreamLine {
        #[serde(default)]
        message: Option<RespMessage>,
        #[serde(default)]
        error: Option<String>,
    }
    let parsed: StreamLine =
        serde_json::from_str(line).map_err(|e| format!("bad stream line: {e}"))?;
    if let Some(reason) = parsed.error {
        return Err(reason);
    }
    Ok(parsed.message.map(|m| m.content).filter(|c| !c.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_config_falls_back_to_defaults() {
        // Neither variable is set in test runs; even if set, the client must
        // still construct. Defaults are asserted only when unset.
        let client = OllamaClient::from_env();
        assert!(!client.model().is_empty());
        if std::env::var("BONSAI_LLM_MODEL").is_err() {
            assert_eq!(client.model(), DEFAULT_MODEL);
        }
    }

    #[test]
    fn chat_request_disables_thinking() {
        let msgs = vec![Msg::user("hi")];
        let req = ChatRequest {
            model: "qwen3.5:2b",
            messages: &msgs,
            stream: false,
            think: false,
            options: Options { temperature: 0.7, num_ctx: 0 },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!(false));
    }

    #[test]
    fn request_sends_the_explicit_context_window() {
        // D5：不再留 0（= Ollama 默认 2k/4k，超窗丢开头=丢人格宪法）。
        let client = OllamaClient::from_env();
        let msgs = vec![Msg::user("hi")];
        let json = serde_json::to_value(client.request(&msgs, 0.7, false)).unwrap();
        assert_eq!(json["options"]["num_ctx"], serde_json::json!(NUM_CTX));
        assert!(NUM_CTX >= 8192, "低于 8192 装不下宪法 + 最近几轮");
    }

    #[test]
    fn stream_line_parses_content_deltas() {
        let line = r#"{"message":{"role":"assistant","content":"你好"},"done":false}"#;
        assert_eq!(parse_stream_line(line).unwrap(), Some("你好".to_string()));
    }

    #[test]
    fn stream_line_ignores_empty_and_final_lines() {
        assert_eq!(parse_stream_line(r#"{"message":{"role":"assistant","content":""},"done":false}"#).unwrap(), None);
        let final_line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        assert_eq!(parse_stream_line(final_line).unwrap(), None);
        assert_eq!(parse_stream_line("").unwrap(), None); // keepalive
    }

    #[test]
    fn stream_line_surfaces_server_errors() {
        let line = r#"{"error":"model not found"}"#;
        assert_eq!(
            parse_stream_line(line).unwrap_err(),
            "model not found".to_string()
        );
    }

    #[test]
    fn is_up_fails_fast_on_closed_port() {
        // Nothing listens here (port 1 on localhost) → probe must return
        // false within the probe timeout, not panic.
        let client = OllamaClient::new("http://127.0.0.1:1", "test");
        assert!(!client.is_up());
    }
}
