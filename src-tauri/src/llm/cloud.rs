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
}
