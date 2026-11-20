//! MCP client core (spec-s1): one wrapper over the official rmcp SDK for
//! both stdio (child process) and Streamable HTTP transports.
//!
//! The async boundary lives INSIDE this module — everything outwards is a
//! sync facade (the codebase is ureq-style sync; tokio is already Tauri's
//! runtime, we just block on it here). Timeouts follow the calendar lesson:
//! hard bounds, never hang the calling thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, Implementation,
};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;

use crate::agent_tools::{clip_chars, mask_secrets, ToolCall, ToolDescriptor, ToolKind, ToolOutput, ToolProvider};

/// Hard timeouts (spec-s1 §2): connect 10s, list/call 60s.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const SUMMARY_MAX: usize = 200;

pub const ERR_TIMEOUT: &str = "mcp_timeout";
pub const ERR_UNREACHABLE: &str = "mcp_unreachable";

/// How one server is reached (spec-s1 §2). Transport-agnostic above this.
#[derive(Debug, Clone)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
    Http { url: String, headers: Vec<(String, String)> },
}

impl McpTransport {
    /// Parse from a server row (spec-s2): `transport` decides which fields
    /// are authoritative.
    pub fn from_row(
        transport: &str,
        command: Option<&str>,
        args_json: &str,
        url: Option<&str>,
        headers_json: &str,
        env_json: &str,
    ) -> Result<McpTransport, String> {
        let parse_kv = |json: &str| -> Vec<(String, String)> {
            serde_json::from_str::<HashMap<String, String>>(json)
                .map(|m| m.into_iter().collect())
                .unwrap_or_default()
        };
        match transport {
            "http" => {
                let url = url.unwrap_or_default().trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err("bad_url".into());
                }
                Ok(McpTransport::Http { url, headers: parse_kv(headers_json) })
            }
            _ => {
                let command = command.unwrap_or_default().trim().to_string();
                if command.is_empty() {
                    return Err("bad_command".into());
                }
                let args = serde_json::from_str::<Vec<String>>(args_json).unwrap_or_default();
                Ok(McpTransport::Stdio { command, args, env: parse_kv(env_json) })
            }
        }
    }
}

fn client_info() -> ClientConfig {
    // ClientConfig (= InitializeRequestParams) is non-exhaustive → constructor.
    ClientConfig::new(ClientCapabilities::default(), Implementation::from_build_env())
}

/// One live MCP connection. Holding the RunningService keeps the child
/// process / HTTP session alive; Drop cancels the service loop.
pub struct McpClient {
    service: RunningService<RoleClient, ClientConfig>,
}

impl McpClient {
    pub fn connect(t: &McpTransport) -> Result<McpClient, String> {
        // 子进程 spawn 必须发生在 tokio runtime 上下文内：rmcp 的
        // TokioChildProcess::new 会立即 spawn（需要活跃 reactor）。本函数跑在
        // Tauri 同步命令线程（主线程）上，直接 new 会 panic → abort（整库崩溃，
        // 2026-11-20 实测：点「启用 MCP」即闪退）。因此 transport 构建挪进 block_on。
        match t {
            McpTransport::Stdio { command, args, env } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                let service = tauri::async_runtime::block_on(async move {
                    let transport = TokioChildProcess::new(cmd)
                        .map_err(|e| format!("{ERR_UNREACHABLE}: {e}"))?;
                    match tokio::time::timeout(CONNECT_TIMEOUT, client_info().serve(transport)).await {
                        Ok(res) => res.map_err(|e| format!("{ERR_UNREACHABLE}: {e}")),
                        Err(_) => Err(ERR_TIMEOUT.to_string()),
                    }
                })?;
                Ok(McpClient { service })
            }
            McpTransport::Http { url, headers } => {
                let transport = http_transport(url, headers)?;
                let service = tauri::async_runtime::block_on(async {
                    tokio::time::timeout(CONNECT_TIMEOUT, client_info().serve(transport)).await
                })
                .map_err(|_| ERR_TIMEOUT.to_string())?
                .map_err(|e| format!("{ERR_UNREACHABLE}: {e}"))?;
                Ok(McpClient { service })
            }
        }
    }

    /// tools/list → descriptors (level 1 summaries trimmed, level 2 detail =
    /// input schema JSON). Sorted by name for a stable catalog.
    pub fn list_tools(&self) -> Result<Vec<ToolDescriptor>, String> {
        let list = tauri::async_runtime::block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, self.service.list_all_tools()).await
        })
        .map_err(|_| ERR_TIMEOUT.to_string())?
        .map_err(|e| format!("{ERR_UNREACHABLE}: {e}"))?;
        let mut out = Vec::new();
        for tool in list {
            let name = tool.name.to_string();
            let summary = tool
                .description
                .as_ref()
                .map(|d| clip_chars(&mask_secrets(d), 40))
                .unwrap_or_default();
            let detail = serde_json::to_string(&tool.input_schema).unwrap_or_default();
            out.push(ToolDescriptor { name, summary, detail, kind: ToolKind::Mcp, remote: false });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// tools/call with a 60s hard timeout. Text content is joined for the
    /// summary (≤200 chars, masked); the full text stays out of the model.
    pub fn call_tool(&self, call: &ToolCall) -> Result<ToolOutput, String> {
        let mut params = CallToolRequestParams::new(call.name.clone());
        params.arguments = Some(call.args.as_object().cloned().unwrap_or_default());
        let result = tauri::async_runtime::block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, self.service.call_tool(params)).await
        })
        .map_err(|_| ERR_TIMEOUT.to_string())?
        .map_err(|e| format!("{ERR_UNREACHABLE}: {e}"))?;
        let mut full = String::new();
        for c in &result.content {
            if let ContentBlock::Text(t) = c {
                full.push_str(&t.text);
                full.push('\n');
            }
        }
        let full = full.trim().to_string();
        if result.is_error.unwrap_or(false) {
            return Err(format!("tool_error: {}", clip_chars(&full, 120)));
        }
        Ok(ToolOutput { summary: clip_chars(&mask_secrets(&full), SUMMARY_MAX), full })
    }

    pub fn ping(&self) -> bool {
        !self.service.is_closed()
    }

    /// Graceful shutdown with a bounded wait (orphan prevention, spec-s1).
    pub fn shutdown(&mut self) {
        let _ = tauri::async_runtime::block_on(self.service.close_with_timeout(SHUTDOWN_TIMEOUT));
    }
}

/// Streamable HTTP transport via the rmcp config builder: `Authorization`
/// goes through `auth_header` (token without the `Bearer ` prefix), other
/// headers land in `custom_headers`.
fn http_transport(
    url: &str,
    headers: &[(String, String)],
) -> Result<rmcp::transport::StreamableHttpClientTransport<reqwest::Client>, String> {
    use http::header::{HeaderName, HeaderValue};
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    let mut custom: HashMap<HeaderName, HeaderValue> = HashMap::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("authorization") {
            // rmcp expects the raw token; strip a "Bearer " prefix if present.
            let token = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")).unwrap_or(v);
            config = config.auth_header(token.to_string());
        } else {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| format!("bad_header: {e}"))?;
            let value = HeaderValue::from_str(v).map_err(|e| format!("bad_header: {e}"))?;
            custom.insert(name, value);
        }
    }
    if !custom.is_empty() {
        config = config.custom_headers(custom);
    }
    Ok(rmcp::transport::StreamableHttpClientTransport::from_config(config))
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------- manager

/// Live connections keyed by server row id, shared across chat turns and
/// admin commands. Refreshed by toggle/save/remove (spec-s2 §2).
#[derive(Default)]
pub struct McpManager {
    clients: Mutex<HashMap<String, Arc<McpClient>>>,
}

impl McpManager {
    pub fn get(&self, server_id: &str) -> Option<Arc<McpClient>> {
        self.clients.lock().ok()?.get(server_id).cloned()
    }

    pub fn insert(&self, server_id: String, client: Arc<McpClient>) {
        if let Ok(mut map) = self.clients.lock() {
            map.insert(server_id, client);
        }
    }

    pub fn remove(&self, server_id: &str) {
        if let Ok(mut map) = self.clients.lock() {
            map.remove(server_id); // Drop → shutdown
        }
    }

    /// Exit-path sweep: bounded shutdown of every live connection.
    pub fn shutdown_all(&self) {
        if let Ok(mut map) = self.clients.lock() {
            map.clear(); // Drop → shutdown
        }
    }
}

// ---------------------------------------------------------------- mock

/// In-memory provider for pipeline tests (spec-s1 §3) and offline demo.
#[derive(Default)]
pub struct MockMcpProvider {
    pub tools: Vec<ToolDescriptor>,
    /// Fixed reply per tool name; missing name → `mcp_unreachable`.
    pub replies: HashMap<String, String>,
    pub fail_for: Vec<String>,
}

impl MockMcpProvider {
    pub fn with_tool(name: &str, summary: &str, detail: &str) -> Self {
        MockMcpProvider {
            tools: vec![ToolDescriptor {
                name: name.into(),
                summary: summary.into(),
                detail: detail.into(),
                kind: ToolKind::Mcp,
                remote: false,
            }],
            ..Default::default()
        }
    }
}

impl ToolProvider for MockMcpProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, String> {
        if self.fail_for.iter().any(|n| n == &call.name) {
            return Err(ERR_TIMEOUT.into());
        }
        match self.replies.get(&call.name) {
            Some(text) => Ok(ToolOutput { summary: clip_chars(text, SUMMARY_MAX), full: text.clone() }),
            None => Err(ERR_UNREACHABLE.into()),
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_outside_runtime_returns_err_not_panic() {
        // 回归（2026-11-20 崩溃）：TokioChildProcess::new 曾在 tokio runtime
        // 上下文外 spawn 子进程 → panic → abort（点「启用」即闪退）。
        // 修复后：spawn 在 block_on 内进行，失败必须是干净的 Err。
        let t = McpTransport::from_row("stdio", Some("/bin/echo"), r#"["hi"]"#, None, "{}", "{}").unwrap();
        let err = match McpClient::connect(&t) {
            Err(e) => e,
            Ok(_) => panic!("expected connect failure, got live client"),
        };
        assert!(
            err.starts_with(ERR_UNREACHABLE) || err == ERR_TIMEOUT,
            "unexpected error: {err}"
        );
    }

    #[test]
    fn transport_from_row_validates_fields() {
        let t = McpTransport::from_row(
            "stdio", Some("/bin/echo"), r#"["hi"]"#, None, "{}", "{}",
        )
        .unwrap();
        assert!(matches!(t, McpTransport::Stdio { .. }));

        let t = McpTransport::from_row(
            "http", None, "[]", Some("https://mcp.example.com/v1"), r#"{"Authorization":"Bearer abc"}"#, "{}",
        )
        .unwrap();
        match t {
            McpTransport::Http { url, headers } => {
                assert_eq!(url, "https://mcp.example.com/v1");
                assert_eq!(headers[0].0, "Authorization");
            }
            _ => panic!("expected http"),
        }

        // stdio without command, http without/with-bad url → rejected
        assert!(McpTransport::from_row("stdio", None, "[]", None, "{}", "{}").is_err());
        assert!(McpTransport::from_row("http", None, "[]", None, "{}", "{}").is_err());
        assert!(McpTransport::from_row("http", None, "[]", Some("ftp://x"), "{}", "{}").is_err());
    }

    #[test]
    fn mock_provider_executes_and_fails() {
        let mut p = MockMcpProvider::with_tool("weather", "查天气", r#"{"type":"object"}"#);
        p.replies.insert("weather".into(), "杭州今天多云 18 度".into());
        p.fail_for.push("weather".to_string());
        let err = p
            .execute(&ToolCall { name: "weather".into(), args: serde_json::json!({"city": "杭州"}) })
            .unwrap_err();
        assert_eq!(err, ERR_TIMEOUT);
        p.fail_for.clear();
        let out = p
            .execute(&ToolCall { name: "weather".into(), args: serde_json::json!({}) })
            .unwrap();
        assert_eq!(out.summary, "杭州今天多云 18 度");
        assert_eq!(p.descriptors()[0].summary, "查天气");
        assert_eq!(p.descriptors()[0].detail, r#"{"type":"object"}"#);
    }

    #[test]
    fn mock_provider_truncates_summary_but_keeps_full_text() {
        // Mirrors McpClient::call_tool: the model sees ≤200 chars, the UI can
        // still show everything (spec-s1 §3 item 3).
        let long = "很长的工具结果".repeat(60);
        let mut p = MockMcpProvider::with_tool("dump", "导出行", "{}");
        p.replies.insert("dump".into(), long.clone());

        let out = p
            .execute(&ToolCall { name: "dump".into(), args: serde_json::json!({}) })
            .unwrap();
        assert_eq!(out.full, long, "full text stays intact");
        assert!(out.full.chars().count() > 200);
        assert!(out.summary.chars().count() <= 200, "summary is clipped");
    }

    #[test]
    fn unknown_tool_is_unreachable_not_empty_success() {
        // Calendar lesson: a missing/failed tool must surface an explicit code,
        // never an empty result that reads like "no data".
        let p = MockMcpProvider::with_tool("weather", "查天气", "{}");
        let err = p
            .execute(&ToolCall { name: "ghost".into(), args: serde_json::json!({}) })
            .unwrap_err();
        assert_eq!(err, ERR_UNREACHABLE);
    }
}
