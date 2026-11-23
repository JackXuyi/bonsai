//! Agent tool pipeline primitives (plan-agent-mcp-skill.md §3.6, spec-s3/s4).
//!
//! One abstraction for every capability the model can reach — MCP tools,
//! skill packs, and future built-in tools (P3-2 files / P3-3 web). The
//! three-stage pipeline, the approval gate, the audit and the UI all speak
//! `ToolProvider` / `ToolDescriptor` and never know where a capability came
//! from.
//!
//! Design guards baked in here:
//! - everything the model may emit is parsed strictly and validated against
//!   the whitelist — a malformed or unknown name is dropped silently;
//! - secret masking happens BEFORE anything is stored or shown.

use serde::Deserialize;

// ---------------------------------------------------------------- kinds

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Mcp,
    Skill,
    /// Built-in, local-only capabilities shipped with the binary (first one:
    /// memory_search). Never remote; audited under the `builtin` kind.
    Builtin,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Mcp => "mcp",
            ToolKind::Skill => "skill",
            ToolKind::Builtin => "builtin",
        }
    }
}

/// Single source of truth for built-in tools (ToolKind::Builtin). The
/// settings page lists these under「内置工具」; the dialogue pipeline mounts
/// them from the same registry, so model view and UI view cannot drift.
/// Each entry delegates to its owning module's `builtin_descriptor` (one
/// definition per tool, referenced here — not duplicated).
pub fn builtin_tools() -> Vec<ToolDescriptor> {
    let mut tools = vec![crate::memory_search::builtin_descriptor()];
    // plan-perception-tool：感知查询工具（内置工具第二枚），descriptor 单源
    // 在 perception_search 模块。设置页常显（让用户知道能力存在）；provider
    // 挂载由 build_agent_kit 按 might_perceive 信号门控。
    tools.push(crate::perception_search::builtin_descriptor());
    // P3-2 文件工具（plan-files.md）：descriptor 单源在 filetools 模块。
    // 注意：设置页常显这四个条目（让用户知道能力存在），但 provider 只在
    // 总开关开且 roots 非空时才挂载（见 mcp_admin::build_agent_kit）。
    tools.extend(crate::filetools::builtin_descriptors());
    // P3-3 Web 工具（plan-web.md）：descriptor 单源在 webtools 模块。设置页
    // 常显这两个条目；provider 只在 tools.web.enabled 时才挂载（见
    // mcp_admin::build_agent_kit）。默认关——搜索词与网页内容会外发，
    // 开启是知情选择。
    tools.extend(crate::webtools::builtin_descriptors());
    tools
}

/// 权限三档（plan-task-card v0.2 §6，roadmap G-10）：只读直行、写操作
/// 强制 ask（无视全局 approval_mode——「危险动作审批覆盖率 100%」）、
/// 危险类永拒。默认 Read：存量工具反序列化旧缓存（无该字段）与所有
/// 既有构造点都落到只读，行为零回归。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolAccess {
    #[default]
    Read,
    Ask,
    Forbidden,
}

impl ToolAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolAccess::Read => "read",
            ToolAccess::Ask => "ask",
            ToolAccess::Forbidden => "forbidden",
        }
    }
}

/// Three disclosure levels in one struct (spec-s3 §1): the pipeline shows
/// `name` by default, `summary` after the name-selection round, `detail`
/// only for the finalists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub summary: String,
    pub detail: String,
    pub kind: ToolKind,
    pub remote: bool,
    /// 权限档（v0.2 §6）。旧 tool_cache 缓存无此字段 → serde default = Read。
    #[serde(default)]
    pub access: ToolAccess,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// ≤200 chars, template-extracted — what the model is allowed to see.
    pub summary: String,
    /// Full text, folded in the frontend, never fed to the model.
    pub full: String,
}

pub trait ToolProvider: Send + Sync {
    fn descriptors(&self) -> Vec<ToolDescriptor>;
    /// Skills and other prompt-only kinds answer `Err("unsupported")` (D4).
    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, String>;
    /// Whether the provider actually READS the connection during execution.
    /// `execute_audited` locks the store only for these providers (built-in
    /// memory_search); MCP providers run their subprocess wait WITHOUT the
    /// DB lock (2026-11-20 lock-narrowing: a 10s tool timeout must not
    /// block every other store consumer).
    fn needs_conn(&self) -> bool {
        false
    }
    /// Built-in tools that need the memory store override this AND
    /// `needs_conn`. Default: conn-less execution, so every existing
    /// provider is untouched.
    fn execute_with_conn(
        &self,
        _conn: &rusqlite::Connection,
        call: &ToolCall,
    ) -> Result<ToolOutput, String> {
        self.execute(call)
    }
}

// ---------------------------------------------------------------- approval

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

pub trait ApprovalGate: Send + Sync {
    /// Called AFTER arg validation, BEFORE execution (spec-s4 §1).
    fn check(&self, call: &ToolCall, source: &str, remote: bool) -> ApprovalDecision;
}

/// Current default (D10): every call is allowed, the audit row says
/// `auto_allowed`. Swapping to AskFirstGate is a settings flip — no
/// call-site changes.
pub struct AutoAllowGate;

impl ApprovalGate for AutoAllowGate {
    fn check(&self, _call: &ToolCall, _source: &str, _remote: bool) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

// ---------------------------------------------------------------- text utils

/// Mask token-like substrings before anything is audited or shown:
/// `Bearer xxx`, `Authorization: xxx` and `sk-…` style keys become `••••tail`.
/// Walks with a cursor so an already-masked token can never re-trigger the
/// same prefix (a `while find` loop would spin forever on `sk-`).
pub fn mask_secrets(s: &str) -> String {
    let mut out = s.to_string();
    for prefix in ["Bearer ", "bearer ", "Authorization: ", "authorization: ", "sk-"] {
        let mut pos = 0usize;
        while let Some(idx) = out[pos..].find(prefix) {
            let start = pos + idx + prefix.len();
            let rest = &out[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == '}')
                .unwrap_or(rest.len());
            let token = &rest[..end];
            if token.is_empty() || token.starts_with('•') {
                pos = start;
                continue;
            }
            // "Authorization: Bearer xxx" — the scheme word is not the secret;
            // the "Bearer " prefix rule already masked the real token.
            if token.eq_ignore_ascii_case("bearer") || token.eq_ignore_ascii_case("token") {
                pos = start + end;
                continue;
            }
            let tail: String = token.chars().rev().take(4).collect();
            let masked = format!("••••{}", tail.chars().rev().collect::<String>());
            out.replace_range(start..start + end, &masked);
            pos = start + masked.len();
        }
    }
    out
}

/// Cap the model-visible summary at `n` chars (spec-s3: 200).
pub fn clip_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Name-only catalog line (level 0): `a · b · c`, ≤100 entries (D-裁决
/// 2026-11-20：spec-s3 从 20 放宽到 100——单机 39 个 MCP 工具时 20 的
/// 截断让尾部工具永远不可见；100 个名字约 600 字符，本地上下文无压力).
pub fn name_catalog(descriptors: &[ToolDescriptor]) -> String {
    descriptors
        .iter()
        .take(100)
        .map(|d| d.name.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---------------------------------------------------------------- model output parsing

/// Level-0 answer shape. The model is instructed to output ONLY this JSON
/// when it wants capabilities, or to answer normally otherwise.
#[derive(Debug, Deserialize)]
pub struct CandidatesReply {
    #[serde(default)]
    pub candidates: Vec<String>,
}

/// Extract the first balanced JSON object from a reply. Local 2B models
/// wrap JSON in prose or code fences often enough that a bare
/// `serde_json::from_str` is too brittle. crate-visible so the dialogue
/// pipeline can tell "prose" from "regurgitated JSON" before trusting it.
pub(crate) fn json_object_slice(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let bytes = reply.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        let c = b as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&reply[start..=start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse the level-0 reply. `Ok(None)` = no valid candidates JSON → the
/// reply itself is the final answer (silent degrade, spec-s3 §3).
pub fn parse_candidates(reply: &str) -> Result<Vec<String>, ()> {
    let slice = json_object_slice(reply).ok_or(())?;
    let parsed: CandidatesReply = serde_json::from_str(slice).map_err(|_| ())?;
    Ok(parsed.candidates)
}

/// Level-1 answer shape: a tool decision, or `{"none": true}` to bail out
/// (legal at any point — e.g. the skill body already answered it).
#[derive(Debug, Deserialize)]
pub struct ToolDecisionReply {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub none: bool,
}

pub enum ToolDecision {
    /// {"none": true} or garbage → answer as-is.
    None,
    Call { name: String, args: serde_json::Value },
}

pub fn parse_tool_decision(reply: &str) -> Result<ToolDecision, ()> {
    let slice = json_object_slice(reply).ok_or(())?;
    let parsed: ToolDecisionReply = serde_json::from_str(slice).map_err(|_| ())?;
    if parsed.none {
        return Ok(ToolDecision::None);
    }
    match (parsed.tool, parsed.args) {
        (Some(name), Some(args)) if !name.trim().is_empty() => {
            Ok(ToolDecision::Call { name: name.trim().to_string(), args })
        }
        _ => Err(()),
    }
}

/// Minimal JSON-Schema subset check (plan §3.3 round 2: strict validation
/// before the gate). Covers the subset MCP servers actually declare —
/// top-level `type: object`, `properties` with primitive types, `required`.
/// Unknown extra fields pass (servers vary widely); an unparseable schema
/// also passes (we cannot validate what we cannot read — execution errors
/// stay the backstop). Returns `bad_args:…` so the audit records the reason.
pub fn validate_args(schema: &str, args: &serde_json::Value) -> Result<(), String> {
    let Ok(schema) = serde_json::from_str::<serde_json::Value>(schema) else {
        return Ok(());
    };
    if schema.get("type").and_then(|t| t.as_str()) != Some("object") {
        return Ok(()); // only object schemas are checked; anything else → trust the server
    }
    let Some(obj) = args.as_object() else {
        return Err("bad_args:not-an-object".into());
    };
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for name in required.iter().filter_map(|v| v.as_str()) {
            if !obj.contains_key(name) {
                return Err(format!("bad_args:missing:{name}"));
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (name, spec) in props {
            let Some(value) = obj.get(name) else { continue };
            let declared = spec.get("type").and_then(|t| t.as_str());
            let ok = match declared {
                Some("string") => value.is_string(),
                Some("boolean") => value.is_boolean(),
                Some("number") => value.is_number(),
                Some("integer") => value.is_i64() || value.is_u64(),
                Some("array") => value.is_array(),
                Some("object") => value.is_object(),
                _ => true, // unknown/absent type → trust the server
            };
            if !ok {
                return Err(format!("bad_args:type:{}", name));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(name: &str, kind: ToolKind) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            summary: format!("{name} 的说明"),
            detail: format!("{name} 的完整说明"),
            kind,
            remote: false,
            access: ToolAccess::Read,
        }
    }

    #[test]
    fn validate_args_covers_required_types_and_unreadable_schema() {
        let schema = r#"{"type":"object","properties":{"city":{"type":"string"},"days":{"type":"integer"},"flag":{"type":"boolean"}},"required":["city"]}"#;

        // readable schema: required missing / wrong type rejected.
        assert_eq!(
            validate_args(schema, &serde_json::json!({"days": 1})).unwrap_err(),
            "bad_args:missing:city"
        );
        assert_eq!(
            validate_args(schema, &serde_json::json!({"city": "杭州", "flag": "yes"})).unwrap_err(),
            "bad_args:type:flag"
        );
        assert_eq!(
            validate_args(schema, &serde_json::json!({"city": "杭州", "days": 1.5})).unwrap_err(),
            "bad_args:type:days"
        );
        // valid: all types match, extra unknown field tolerated.
        assert!(validate_args(
            schema,
            &serde_json::json!({"city": "杭州", "days": 3, "flag": true, "extra": [1]})
        )
        .is_ok());

        // unreadable / non-object schema → pass-through (execution is the backstop).
        assert!(validate_args("not json", &serde_json::json!({})).is_ok());
        assert!(validate_args("{}", &serde_json::json!({"anything": 1})).is_ok());
        assert!(validate_args(r#"{"type":"array"}"#, &serde_json::json!("x")).is_ok());
    }

    #[test]
    fn name_catalog_lists_up_to_100() {
        let d: Vec<_> = (0..120).map(|i| desc(&format!("t{i:03}"), ToolKind::Mcp)).collect();
        let cat = name_catalog(&d);
        assert!(cat.starts_with("t000 · t001"));
        assert!(cat.ends_with("t099"));
        assert!(!cat.contains("t100"), "only 100 entries");
    }

    #[test]
    fn mask_secrets_covers_bearer_and_sk() {
        assert_eq!(mask_secrets("Authorization: Bearer abcdef1234 done"), "Authorization: Bearer ••••1234 done");
        assert_eq!(mask_secrets("key is sk-abcdef123456"), "key is sk-••••3456");
        let logged = mask_secrets("args {\"auth\": \"Bearer zzzzsecret9\"}");
        assert!(!logged.contains("zzzzsecret9"));
    }

    #[test]
    fn parse_candidates_handles_wrapped_json_and_garbage() {
        assert_eq!(
            parse_candidates("好的 {\"candidates\": [\"a\", \"b\"]} 就这些").unwrap(),
            vec!["a", "b"]
        );
        assert_eq!(
            parse_candidates("```json\n{\"candidates\": [\"weather\"]}\n```").unwrap(),
            vec!["weather"]
        );
        assert!(parse_candidates("我今天不需要工具，直接说吧。").is_err());
        assert!(parse_candidates("{\"candidates\": []}").unwrap().is_empty());
    }

    #[test]
    fn parse_tool_decision_covers_none_call_and_garbage() {
        let d = parse_tool_decision("{\"tool\": \"weather\", \"args\": {\"city\": \"杭州\"}}").unwrap();
        match d {
            ToolDecision::Call { name, args } => {
                assert_eq!(name, "weather");
                assert_eq!(args["city"], "杭州");
            }
            _ => panic!("expected call"),
        }
        assert!(matches!(parse_tool_decision("{\"none\": true}").unwrap(), ToolDecision::None));
        // Prose answer after seeing skill detail → None-ish garbage → Err
        assert!(parse_tool_decision("根据技能包内容，答案是……").is_err());
    }

    #[test]
    fn clip_chars_counts_graphemes_not_bytes() {
        assert_eq!(clip_chars("你好世界", 3), "你好世");
        assert_eq!(clip_chars("abcdef", 200), "abcdef");
    }
}
