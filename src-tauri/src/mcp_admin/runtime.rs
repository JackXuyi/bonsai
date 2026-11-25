//! Agent kit assembly, audited execution, and dialogue cancel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rusqlite::Connection;

use crate::agent_tools::{
    format_args_summary, ApprovalDecision, ApprovalGate, ToolCall, ToolDescriptor,
    ToolOutput, ToolProvider,
};
use crate::mcp::{McpClient, ERR_UNREACHABLE};
use crate::memory::db;

use super::approval::{ApprovalBroker, AskFirstGate};
use super::servers::list_raw;
use super::state::{AgentState, SessionToolAllow};

// ---------------------------------------------------------------- providers

/// ToolProvider over one MCP server row: descriptors from the cache
/// (cheap), execution through the live client (or unreachable).
struct McpToolProvider {
    server_name: String,
    descriptors: Vec<ToolDescriptor>,
    client: Option<Arc<McpClient>>,
}

impl ToolProvider for McpToolProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.descriptors.clone()
    }

    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, String> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| format!("{ERR_UNREACHABLE}: {} 未连接", self.server_name))?;
        // `remote` is carried by the descriptor (from the cache), so the
        // output itself stays transport-agnostic.
        client.call_tool(call)
    }
}

/// Everything one chat turn needs: providers (MCP servers + skills) and the
/// approval gate. Built per turn from managed state; empty providers ⇒ the
/// pipeline stays fully out of the turn (legacy behavior).
pub struct AgentKit {
    pub providers: Vec<Box<dyn ToolProvider>>,
    pub gate: Box<dyn ApprovalGate>,
    /// 权限三档的 ask 档专用门（plan-task-card v0.2 §6）：write 工具
    /// 无视全局 approval_mode 强制走它——「危险动作审批覆盖率 100%」。
    /// 无 AppHandle 时（headless/单测）它拒绝一切（沉默不得变成同意）。
    pub ask_gate: Box<dyn ApprovalGate>,
    /// 任务卡外壳（v12 §8）：云端循环创建 task 后写入，审计行随之关联
    /// `tool_calls.task_id`。本地/无任务路径恒 None。
    pub task_id: Mutex<Option<String>>,
    pub prompt_trace: Mutex<Vec<String>>,
    /// 引擎型 skill 包（plan-summary-skill）：summary.rs 在命中总结意图时
    /// 定向取用。它们不进 `providers`——两个包会占满 SIMULTANEOUS_SKILLS，
    /// 把用户自己的 skill 挤掉。
    pub engine_skills: Vec<crate::skill::SkillPack>,
    /// 轮内工具结果缓存（plan-tool-cache v2）：随 kit 生、随 kit 死——
    /// `build_agent_kit` 每轮新建，调用结束即弃。只对 Read 档生效，
    /// 详见 `crate::tool_cache`。
    pub tool_cache: crate::tool_cache::TurnToolCache,
    /// T4 取消标志：与全局 `CANCEL_FLAGS` 中 task_id 对应的 Arc 共享，
    /// `dialogue_cancel` 命令置 true，云端循环每轮/每步前检查。默认独立
    /// 标志（无任务/本地路径永不取消）。
    pub canceled: Arc<AtomicBool>,
}

/// T4 取消注册表：task_id → 取消标志。命令与执行循环跨线程共享同一个
/// Arc<AtomicBool>。用 OnceLock + RwLock（Rust 1.77 无 LazyLock；沿
/// llm_admin.rs 的 OnceLock 先例）。生命周期由循环收尾时 unregister 清理。
static CANCEL_FLAGS: OnceLock<RwLock<std::collections::HashMap<String, Arc<AtomicBool>>>> =
    OnceLock::new();

fn cancel_registry() -> &'static RwLock<std::collections::HashMap<String, Arc<AtomicBool>>> {
    CANCEL_FLAGS.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

/// 开卡时注册取消标志，返回该 task 的共享 Arc。
pub fn register_cancel(task_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut m) = cancel_registry().write() {
        m.insert(task_id.to_string(), flag.clone());
    }
    flag
}

/// 收尾时注销，防止注册表随任务数无限增长。
pub fn unregister_cancel(task_id: &str) {
    if let Ok(mut m) = cancel_registry().write() {
        m.remove(task_id);
    }
}

/// Tauri 命令：前端「停止」按钮调用；task_id 不存在时静默 OK（幂等）。
#[tauri::command]
pub fn dialogue_cancel(task_id: String) -> Result<(), String> {
    if let Ok(m) = cancel_registry().read() {
        if let Some(flag) = m.get(&task_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn cancel_requested(flag: &Arc<AtomicBool>) -> bool {
    flag.load(Ordering::SeqCst)
}

impl AgentKit {
    pub fn empty() -> Self {
        AgentKit {
            providers: Vec::new(),
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(
                Arc::new(ApprovalBroker::default()),
                None,
                Arc::new(SessionToolAllow::default()),
            )),
            task_id: Mutex::new(None),
            prompt_trace: Mutex::new(Vec::new()),
            engine_skills: Vec::new(),
            tool_cache: crate::tool_cache::TurnToolCache::new(),
            canceled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn has_capabilities(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Full descriptor catalog, sorted, UNtruncated. The whitelist / label
    /// lookups use this: the ≤100 display cap must not execution-bar tail
    /// tools (any tool past the cap that the model names anyway must stay
    /// callable).
    pub fn catalog_all(&self) -> Vec<ToolDescriptor> {
        let mut all = Vec::new();
        for p in &self.providers {
            all.extend(p.descriptors());
        }
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Flat descriptor catalog (≤100, stable order) for the level-0 prompt.
    /// spec-s3 的 20→100 放宽见 name_catalog 注释（2026-11-20 裁决）。
    pub fn catalog(&self) -> Vec<ToolDescriptor> {
        let mut all = self.catalog_all();
        all.truncate(100);
        all
    }

    pub fn trace(&self, line: String) {
        if let Ok(mut v) = self.prompt_trace.lock() {
            v.push(line);
        }
    }

    pub fn trace_ref(&self) -> String {
        self.prompt_trace
            .lock()
            .map(|v| {
                if v.is_empty() {
                    "no-selection".to_string()
                } else {
                    crate::agent_tools::clip_chars(&v.join(" | "), 200)
                }
            })
            .unwrap_or_else(|_| "no-selection".into())
    }
}

/// Find which provider owns a tool name (search in catalog order).
fn find_owner<'a>(
    kit: &'a AgentKit,
    name: &str,
) -> Option<(usize, &'a dyn ToolProvider, ToolDescriptor)> {
    for (idx, p) in kit.providers.iter().enumerate() {
        for d in p.descriptors() {
            if d.name == name {
                return Some((idx, p.as_ref(), d));
            }
        }
    }
    None
}

/// 任务卡关联（v12）：kit.task_id 的只读快照，audit 行落库用。
fn kit_task_id(kit: &AgentKit) -> Option<String> {
    kit.task_id.lock().ok().and_then(|g| g.clone())
}

/// Execute a validated call through the gate + provider, fully audited.
/// The store lock is held ONLY for audit-row writes and for providers that
/// declare `needs_conn()` — MCP subprocess execution (up to ~10s timeout)
/// runs conn-less so a slow tool never blocks other store consumers
/// (2026-11-20 lock-narrowing).
pub fn execute_audited(
    store: &crate::memory::MemoryStore,
    kit: &AgentKit,
    name: &str,
    args: serde_json::Value,
) -> Result<ToolOutput, String> {
    let (_idx, provider, descriptor) =
        find_owner(kit, name).ok_or_else(|| "unknown_tool".to_string())?;
    let args_summary = format_args_summary(name, &args);
    log::debug!(
        "agent_tool_call: tool={} kind={} args={} trace={}",
        name,
        descriptor.kind.as_str(),
        args_summary,
        kit.trace_ref()
    );
    // Strict-args validation (plan §3.3): parameter validation BEFORE the
    // gate — garbage from a small model never reaches approval or execution.
    let started = std::time::Instant::now();
    if let Err(reason) = crate::agent_tools::validate_args(&descriptor.detail, &args) {
        audit_log(
            store,
            descriptor.kind.as_str(),
            &descriptor.name,
            "error",
            &reason,
            Some(&args_summary),
            Some(kit.trace_ref().as_str()),
            descriptor.remote,
            Some(started.elapsed().as_millis() as i64),
            kit_task_id(kit).as_deref(),
        );
        return Err(reason);
    }
    let call = ToolCall { name: name.to_string(), args };
    // 权限三档（plan-task-card v0.2 §6）：forbidden 永拒（不进审批）；
    // ask 档无视全局 approval_mode 强制走 ask_gate；read 走全局门。
    if descriptor.access == crate::agent_tools::ToolAccess::Forbidden {
        audit_log(
            store,
            descriptor.kind.as_str(),
            &descriptor.name,
            "denied",
            "forbidden_tool",
            Some(&args_summary),
            Some(kit.trace_ref().as_str()),
            descriptor.remote,
            Some(started.elapsed().as_millis() as i64),
            kit_task_id(kit).as_deref(),
        );
        return Err("forbidden".into());
    }
    let gate: &dyn ApprovalGate = match descriptor.access {
        crate::agent_tools::ToolAccess::Ask => kit.ask_gate.as_ref(),
        _ => kit.gate.as_ref(),
    };
    let decision = gate.check(&call, &descriptor.name, descriptor.remote);
    if decision == ApprovalDecision::Deny {
        audit_log(
            store,
            descriptor.kind.as_str(),
            &descriptor.name,
            "denied",
            "denied_by_gate",
            Some(&args_summary),
            Some(kit.trace_ref().as_str()),
            descriptor.remote,
            Some(started.elapsed().as_millis() as i64),
            kit_task_id(kit).as_deref(),
        );
        return Err("denied".into());
    }
    let gate_status = if descriptor.access == crate::agent_tools::ToolAccess::Ask {
        "asked_allowed"
    } else if descriptor.remote {
        "auto_allowed_remote"
    } else {
        "auto_allowed"
    };
    // 轮内缓存（plan-tool-cache v2）：Read 档在 gate 照常通过后查本轮
    // 记忆——同 (tool, args) 的第二次调用不再真执行，但 gate/审批语义
    // 零绕过。Ask/Forbidden 永不读缓存（每次执行都要见 gate）。
    if descriptor.access == crate::agent_tools::ToolAccess::Read {
        if let Some(out) = kit.tool_cache.get(name, &call.args) {
            audit_log(
                store,
                descriptor.kind.as_str(),
                &descriptor.name,
                "ok",
                "cache_hit",
                Some(&args_summary),
                Some(kit.trace_ref().as_str()),
                descriptor.remote,
                Some(started.elapsed().as_millis() as i64),
                kit_task_id(kit).as_deref(),
            );
            return Ok(out);
        }
    }
    let result = if provider.needs_conn() {
        let conn = store.write_guard().map_err(|_| "store_poisoned".to_string())?;
        provider.execute_with_conn(&conn, &call)
    } else {
        provider.execute(&call)
    };
    let duration = Some(started.elapsed().as_millis() as i64);
    match result {
        Ok(mut out) => {
            // P4-3：外部/工具正文含注入特征 → 降级声明 + security 留痕（E1）。
            let (summary, hit) = crate::injection::degrade_if_injection(&out.summary);
            out.summary = summary;
            let result_detail = if let Some(pattern) = hit {
                if let Ok(conn) = store.write_guard() {
                    crate::injection::record_flag(&conn, name, pattern);
                }
                "ok_injection_flagged"
            } else {
                gate_status
            };
            audit_log(
                store,
                descriptor.kind.as_str(),
                &descriptor.name,
                "ok",
                result_detail,
                Some(&args_summary),
                Some(kit.trace_ref().as_str()),
                descriptor.remote,
                duration,
                kit_task_id(kit).as_deref(),
            );
            // 缓存写入/失效（plan-tool-cache §2）：Read 成功入缓存；
            // 非 Read（写/ask 档）成功 → 清空整表，写后读必须新鲜。
            if descriptor.access == crate::agent_tools::ToolAccess::Read {
                kit.tool_cache.put(name, &call.args, out.clone());
            } else {
                kit.tool_cache.clear();
            }
            Ok(out)
        }
        Err(e) => {
            let status = if e.contains("mcp_timeout") {
                "mcp_timeout"
            } else if e.contains("mcp_unreachable") {
                "mcp_unreachable"
            } else if e == "unsupported" {
                "unsupported"
            } else {
                "error"
            };
            audit_log(
                store,
                descriptor.kind.as_str(),
                &descriptor.name,
                if status == "mcp_unreachable" { "unavailable" } else { "error" },
                status,
                Some(&args_summary),
                Some(kit.trace_ref().as_str()),
                descriptor.remote,
                duration,
                kit_task_id(kit).as_deref(),
            );
            Err(e)
        }
    }
}

/// One audit row under a short-lived lock. Audit failures never fail the
/// call itself (same semantics as the previous inline `.ok()` writes).
#[allow(clippy::too_many_arguments)]
fn audit_log(
    store: &crate::memory::MemoryStore,
    kind: &str,
    name: &str,
    outcome: &str,
    detail: &str,
    args_summary: Option<&str>,
    trace: Option<&str>,
    remote: bool,
    duration_ms: Option<i64>,
    task_id: Option<&str>,
) {
    let Ok(conn) = store.write_guard() else {
        log::warn!("audit_log: store lock poisoned — row dropped");
        return;
    };
    db::log_agent_tool_call(
        &conn,
        kind,
        name,
        outcome,
        detail,
        args_summary,
        trace,
        remote,
        duration_ms,
        task_id,
    )
    .ok();
}

/// Build the turn's kit from managed state + settings. Cost: one SQL read
/// per turn (settings + enabled servers); live clients come from the pool.
///
/// `user_text` gates the built-in memory_search tool: it is offered only
/// when the utterance carries a recall signal (date words / recall verbs,
/// see memory_search::might_recall). Without the signal — and with no
/// MCP/skill capabilities — the kit stays empty and the turn keeps the
/// legacy single-round chat path.
/// `cloud`：当前回合是否具备云端 function calling（`tool_client` 非空）。
/// 日历 / 邮件 **仅**在云端且对应开关开时挂载——本地零规则腿、零代答。
pub fn build_agent_kit(
    conn: &Connection,
    state: &AgentState,
    user_text: &str,
    calendar_access: crate::calendar::CalendarAccess,
    cloud: bool,
) -> AgentKit {
    let mut providers: Vec<Box<dyn ToolProvider>> = Vec::new();

    if crate::memory_search::might_recall(user_text) {
        providers.push(Box::new(crate::memory_search::MemorySearchProvider));
    }

    // plan-perception-tool：感知查询工具。独立门控 might_perceive（词表语义
    // 与 recall 不同，不并入 might_recall）；本地路径由 dialogue.rs 的
    // local_perception_answer 走同一 kit 的代码层强制路由。
    if crate::perception_search::might_perceive(user_text) {
        providers.push(Box::new(crate::perception_search::PerceptionSearchProvider));
    }

    // P3-2 文件工具（plan-files.md §3.5）：总开关开 + roots 非空才挂载。
    // 空授权 = provider 不进 kit = tools[] 里模型根本看不到 fs_*——
    // 「最小授权」在挂载层生效，而不是执行层报错。
    if crate::filetools::fs_enabled(conn) {
        let roots = crate::filetools::load_roots(conn);
        if !roots.is_empty() {
            let cloud_read = crate::filetools::cloud_read_enabled(conn);
            providers.push(Box::new(crate::filetools::FileToolsProvider::new(roots, cloud_read)));
        }
    }

    // P3-3 Web 检索（plan-web.md §3.6）：开关在挂载层生效，关闭时 tools[]
    // 里根本看不到 web 工具。默认关（搜索词/网页内容会外发，知情开启）。
    if crate::webtools::web_enabled(conn) {
        providers.push(Box::new(crate::webtools::WebToolsProvider::new()));
    }

    // 日历 / 邮件：仅云端 FC（2026-11-25 退役对话规则腿）。
    if cloud && crate::calendar::is_enabled(conn) {
        providers.push(Box::new(crate::tools::CalendarToolsProvider::new(
            calendar_access,
        )));
    }
    if cloud && crate::mail::is_enabled(conn) {
        providers.push(Box::new(crate::mail::MailToolsProvider::apple()));
    }

    // Skills first (prompt-only), then MCP tools — catalog order is stable
    // either way (sorted by name in AgentKit::catalog).
    // Engine packs are loaded separately and handed to summary.rs: they must
    // never enter the generic skill section (two of them fill the whole
    // SIMULTANEOUS_SKILLS budget). Two `stat`s when absent, so the common
    // path stays cheap.
    let engine_skills = crate::skill::load_engine_packs(&state.skills_dir);

    let enabled = crate::skill::enabled_ids(db::get_setting(conn, "skill_enabled").ok().flatten());
    if !enabled.is_empty() {
        let packs: Vec<crate::skill::SkillPack> = crate::skill::scan_skills(&state.skills_dir)
            .into_iter()
            .filter_map(|r| r.ok())
            .filter(|p| enabled.iter().any(|id| id == &p.id))
            .filter(|p| !crate::skill::is_engine_pack(&p.id))
            .collect();
        if !packs.is_empty() {
            providers.push(Box::new(crate::skill::SkillProvider { packs }));
        }
    }

    for row in list_raw(conn).unwrap_or_default().into_iter().filter(|r| r.enabled) {
        let descriptors: Vec<ToolDescriptor> = row
            .tool_cache
            .as_ref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        if descriptors.is_empty() {
            continue;
        }
        // Remote-ness is stamped onto each descriptor's `remote` at cache
        // time, so the provider doesn't need it again.
        let remote = row.transport == "http";
        let descriptors: Vec<ToolDescriptor> = descriptors
            .into_iter()
            .map(|mut d| {
                d.remote = remote;
                d
            })
            .collect();
        providers.push(Box::new(McpToolProvider {
            server_name: row.name.clone(),
            descriptors,
            client: state.manager.get(&row.id),
        }));
    }

    let gate: Box<dyn ApprovalGate> = match db::get_setting(conn, "approval_mode").ok().flatten().as_deref() {
        Some("ask") => Box::new(AskFirstGate::new(
            state.approvals.clone(),
            state.app.clone(),
            state.session_allow.clone(),
        )),
        _ => Box::new(crate::agent_tools::AutoAllowGate),
    };
    // ask 档专用门永远存在（write 工具无视 approval_mode 强制询问）。
    let ask_gate: Box<dyn ApprovalGate> = Box::new(AskFirstGate::new(
        state.approvals.clone(),
        state.app.clone(),
        state.session_allow.clone(),
    ));

    AgentKit { providers, gate, ask_gate, task_id: Mutex::new(None), prompt_trace: Mutex::new(Vec::new()), engine_skills, tool_cache: crate::tool_cache::TurnToolCache::new(), canceled: Arc::new(AtomicBool::new(false)) }
}
