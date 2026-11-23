//! MCP server administration + the agent kit assembly (spec-s2/s4).
//!
//! SQL for `mcp_servers` lives here (agent-owned table); `tool_calls`
//! primitives stay in memory/db.rs. Everything the frontend receives has
//! secrets masked — plaintext headers/env only ever exist inside the DB
//! and the live transport construction.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::agent_tools::{
    clip_chars, mask_secrets, ApprovalDecision, ApprovalGate, ToolCall, ToolDescriptor, ToolOutput,
    ToolProvider,
};
use crate::memory::db;
use crate::mcp::{McpClient, McpManager, McpTransport, ERR_UNREACHABLE};

pub const TOOL_CACHE_TTL_SECS: i64 = 300; // calendar_cache 惯例
/// How long `ask` mode waits for the user before treating silence as denial.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------- rows

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// Masked view of headers_json (frontend never sees plaintext tokens).
    pub headers: Vec<(String, String)>,
    pub env: Vec<(String, String)>,
    pub enabled: bool,
    pub tool_count: usize,
    pub tool_cache_at: Option<String>,
}

fn row_from(conn: &Connection, id: &str) -> Result<RawRow, String> {
    conn.query_row(
        "SELECT id, name, transport, command, args_json, url, headers_json, env_json, enabled,
                tool_cache, tool_cache_at
         FROM mcp_servers WHERE id = ?1",
        [id],
        |r| {
            Ok(RawRow {
                id: r.get(0)?,
                name: r.get(1)?,
                transport: r.get(2)?,
                command: r.get(3)?,
                args_json: r.get(4)?,
                url: r.get(5)?,
                headers_json: r.get(6)?,
                env_json: r.get(7)?,
                enabled: r.get::<_, i64>(8)? != 0,
                tool_cache: r.get(9)?,
                tool_cache_at: r.get(10)?,
            })
        },
    )
    .map_err(|_| "server_not_found".to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct RawRow {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args_json: String,
    pub url: Option<String>,
    pub headers_json: String,
    pub env_json: String,
    pub enabled: bool,
    pub tool_cache: Option<String>,
    pub tool_cache_at: Option<String>,
}

fn parse_kv(json: &str) -> Vec<(String, String)> {
    serde_json::from_str::<std::collections::HashMap<String, String>>(json)
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn masked_kv(json: &str) -> Vec<(String, String)> {
    parse_kv(json)
        .into_iter()
        .map(|(k, v)| (k, mask_secrets(&v)))
        .collect()
}

/// Dedup key for auto-discovery (mcp_discover): must match
/// `DiscoveredServer::signature` byte-for-byte.
pub(crate) fn row_signature(r: &RawRow) -> String {
    if r.transport == "http" {
        format!("http|{}", r.url.clone().unwrap_or_default())
    } else {
        let args: Vec<String> = serde_json::from_str(&r.args_json).unwrap_or_default();
        format!(
            "stdio|{}|{}",
            r.command.clone().unwrap_or_default(),
            args.join("\u{1f}")
        )
    }
}

fn view(row: &RawRow, conn: &Connection) -> McpServerRow {
    let tool_count = row
        .tool_cache
        .as_ref()
        .and_then(|c| serde_json::from_str::<Vec<ToolDescriptor>>(c).ok())
        .map(|v| v.len())
        .unwrap_or(0);
    let _ = conn;
    McpServerRow {
        id: row.id.clone(),
        name: row.name.clone(),
        transport: row.transport.clone(),
        command: row.command.clone(),
        args: serde_json::from_str(&row.args_json).unwrap_or_default(),
        url: row.url.clone(),
        headers: masked_kv(&row.headers_json),
        env: masked_kv(&row.env_json),
        enabled: row.enabled,
        tool_count,
        tool_cache_at: row.tool_cache_at.clone(),
    }
}

// ---------------------------------------------------------------- SQL

pub(crate) fn list_raw(conn: &Connection) -> rusqlite::Result<Vec<RawRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, transport, command, args_json, url, headers_json, env_json, enabled,
                tool_cache, tool_cache_at
         FROM mcp_servers ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RawRow {
            id: r.get(0)?,
            name: r.get(1)?,
            transport: r.get(2)?,
            command: r.get(3)?,
            args_json: r.get(4)?,
            url: r.get(5)?,
            headers_json: r.get(6)?,
            env_json: r.get(7)?,
            enabled: r.get::<_, i64>(8)? != 0,
            tool_cache: r.get(9)?,
            tool_cache_at: r.get(10)?,
        })
    })?;
    rows.collect()
}

pub(crate) fn insert_server(
    conn: &Connection,
    name: &str,
    transport: &str,
    command: Option<&str>,
    args: &[String],
    url: Option<&str>,
    headers: &[(String, String)],
    env: &[(String, String)],
) -> Result<String, String> {
    validate_transport(transport, command, url)?;
    let id = uuid_v4();
    conn.execute(
        "INSERT INTO mcp_servers
           (id, name, transport, command, args_json, url, headers_json, env_json, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        rusqlite::params![
            id,
            name,
            transport,
            command,
            serde_json::to_string(args).unwrap_or_default(),
            url,
            serde_json::to_string(&headers.iter().cloned().collect::<std::collections::HashMap<_, _>>())
                .unwrap_or_default(),
            serde_json::to_string(&env.iter().cloned().collect::<std::collections::HashMap<_, _>>())
                .unwrap_or_default(),
            crate::memory::clock::now_iso(),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

fn update_server(conn: &Connection, id: &str, name: &str, transport: &str, command: Option<&str>, args: &[String], url: Option<&str>, headers: &[(String, String)], env: &[(String, String)]) -> Result<(), String> {
    validate_transport(transport, command, url)?;
    let n = conn
        .execute(
            "UPDATE mcp_servers SET name=?2, transport=?3, command=?4, args_json=?5, url=?6,
                    headers_json=?7, env_json=?8, enabled=0, tool_cache=NULL, tool_cache_at=NULL
             WHERE id=?1",
            rusqlite::params![
                id,
                name,
                transport,
                command,
                serde_json::to_string(args).unwrap_or_default(),
                url,
                serde_json::to_string(&headers.iter().cloned().collect::<std::collections::HashMap<_, _>>())
                    .unwrap_or_default(),
                serde_json::to_string(&env.iter().cloned().collect::<std::collections::HashMap<_, _>>())
                    .unwrap_or_default(),
            ],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        // db 层 UPDATE 未命中 = 没这行（踩坑记录：静默落空）——显式报错。
        return Err("server_not_found".into());
    }
    Ok(())
}

fn validate_transport(transport: &str, command: Option<&str>, url: Option<&str>) -> Result<(), String> {
    match transport {
        "http" => {
            let url = url.unwrap_or_default().trim();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("bad_url".into());
            }
        }
        _ => {
            if command.unwrap_or_default().trim().is_empty() {
                return Err("bad_command".into());
            }
        }
    }
    Ok(())
}

/// Compact uuid (no external crate: rand not in the tree; this is enough
/// entropy for a locally-stored config row).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id() as u128;
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as u128;
    let mut x = nanos ^ (pid << 64) ^ (seq << 96) ^ 0x9E3779B97F4A7C15;
    let mut hex = String::new();
    for i in 0..32 {
        if matches!(i, 8 | 12 | 16 | 20) {
            hex.push('-');
        }
        // xorshift-ish mixing per nibble
        if i % 4 == 0 && i > 0 {
            x = x.rotate_left(17) ^ 0xA5A5A5A5A5A5A5A5;
        }
        let nibble = ((x >> ((i % 16) * 4)) & 0xF) as u8;
        hex.push(std::char::from_digit(nibble as u32, 16).unwrap_or('0'));
    }
    hex
}

/// 任务卡 id（v12 外壳：tasks 表主键，纯本地标识）。
pub fn new_task_id() -> String {
    uuid_v4()
}

static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// ---------------------------------------------------------------- runtime state

/// Managed app state: live MCP connections + the skills directory.
pub struct AgentState {
    pub manager: McpManager,
    pub skills_dir: PathBuf,
    /// `approval_mode=ask` handshake slots (spec-s4). Separate from the DB
    /// on purpose: the chat turn holds the store mutex while it waits.
    pub approvals: Arc<ApprovalBroker>,
    /// `None` in unit tests → ask mode denies instead of waiting forever.
    pub app: Option<tauri::AppHandle>,
}

/// Refresh tool_cache for one server row (shared by toggle / test / startup
/// reconnect).
fn write_tool_cache(conn: &Connection, id: &str, descriptors: &[ToolDescriptor]) -> Result<(), String> {
    conn.execute(
        "UPDATE mcp_servers SET tool_cache=?2, tool_cache_at=?3 WHERE id=?1",
        rusqlite::params![
            id,
            serde_json::to_string(&descriptors).unwrap_or_default(),
            crate::memory::clock::now_iso()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Build a transport row → live client, refresh tool_cache. Used by toggle
/// and test-connect.
fn connect_and_cache(conn: &Connection, row: &RawRow) -> Result<(Arc<McpClient>, Vec<ToolDescriptor>), String> {
    let transport = McpTransport::from_row(
        &row.transport,
        row.command.as_deref(),
        &row.args_json,
        row.url.as_deref(),
        &row.headers_json,
        &row.env_json,
    )?;
    let client = Arc::new(McpClient::connect(&transport)?);
    let descriptors = client.list_tools()?;
    write_tool_cache(conn, &row.id, &descriptors)?;
    Ok((client, descriptors))
}

/// Reconnect every ENABLED server at app startup (2026-11-20 getHardwareInfo
/// regression: the pool is only filled by the admin toggle, so any restart
/// left enabled servers "catalog visible but not connected" — the model
/// named a tool, execute_audited found no client, the turn answered
/// 「没能执行成功」). Best effort per row: a slow/hung server must not stall
/// the others or boot, so connect happens OUTSIDE the db lock; failures are
/// logged and skipped (the tool stays listed but calls will surface
/// mcp_unreachable, which the chat template now explains). Returns the
/// number of servers successfully reconnected.
pub fn reconnect_enabled_servers(agent: &AgentState, store: &crate::memory::MemoryStore) -> usize {
    let rows: Vec<RawRow> = {
        let conn = match store.0.lock() {
            Ok(c) => c,
            Err(_) => return 0,
        };
        list_raw(&conn)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.enabled)
            .collect()
    };
    let mut ok = 0;
    for row in rows {
        let transport = match McpTransport::from_row(
            &row.transport,
            row.command.as_deref(),
            &row.args_json,
            row.url.as_deref(),
            &row.headers_json,
            &row.env_json,
        ) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("mcp_reconnect {}: bad transport: {e}", row.name);
                continue;
            }
        };
        let client = match McpClient::connect(&transport) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("mcp_reconnect {}: connect failed: {e}", row.name);
                continue;
            }
        };
        let descriptors = match client.list_tools() {
            Ok(d) => d,
            Err(e) => {
                log::warn!("mcp_reconnect {}: list_tools failed: {e}", row.name);
                continue;
            }
        };
        {
            let conn = match store.0.lock() {
                Ok(c) => c,
                Err(_) => continue,
            };
            let _ = write_tool_cache(&conn, &row.id, &descriptors);
        }
        log::info!("mcp_reconnect: {} up ({} tools)", row.name, descriptors.len());
        agent.manager.insert(row.id.clone(), Arc::new(client));
        ok += 1;
    }
    ok
}

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
            ask_gate: Box::new(AskFirstGate::new(Arc::new(ApprovalBroker::default()), None)),
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
    let args_summary = crate::agent_tools::clip_chars(&mask_secrets(&args.to_string()), 200);
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
        let conn = store.0.lock().map_err(|_| "store_poisoned".to_string())?;
        provider.execute_with_conn(&conn, &call)
    } else {
        provider.execute(&call)
    };
    let duration = Some(started.elapsed().as_millis() as i64);
    match result {
        Ok(out) => {
            audit_log(
                store,
                descriptor.kind.as_str(),
                &descriptor.name,
                "ok",
                gate_status,
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
    let Ok(conn) = store.0.lock() else {
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
pub fn build_agent_kit(conn: &Connection, state: &AgentState, user_text: &str) -> AgentKit {
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
        Some("ask") => Box::new(AskFirstGate::new(state.approvals.clone(), state.app.clone())),
        _ => Box::new(crate::agent_tools::AutoAllowGate),
    };
    // ask 档专用门永远存在（write 工具无视 approval_mode 强制询问）。
    let ask_gate: Box<dyn ApprovalGate> =
        Box::new(AskFirstGate::new(state.approvals.clone(), state.app.clone()));

    AgentKit { providers, gate, ask_gate, task_id: Mutex::new(None), prompt_trace: Mutex::new(Vec::new()), engine_skills, tool_cache: crate::tool_cache::TurnToolCache::new(), canceled: Arc::new(AtomicBool::new(false)) }
}

/// One-at-a-time approval handshake (spec-s4): the chat turn reserves a slot
/// and blocks on it; the UI answers through `mcp_approval_reply`.
#[derive(Default)]
pub struct ApprovalBroker {
    inner: Mutex<BrokerInner>,
}

#[derive(Default)]
struct BrokerInner {
    seq: u64,
    pending: Option<Pending>,
}

struct Pending {
    id: u64,
    slot: Arc<(Mutex<Option<bool>>, Condvar)>,
}

type Slot = Arc<(Mutex<Option<bool>>, Condvar)>;

impl ApprovalBroker {
    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerInner> {
        // A poisoned broker must not turn into a silent allow — recover the
        // guard and keep the (still conservative) Deny behavior.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Reserve the slot; returns the id shown on the card plus the wait slot.
    fn register(&self) -> (u64, Slot) {
        let mut inner = self.lock();
        inner.seq += 1;
        let id = inner.seq;
        let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
        inner.pending = Some(Pending { id, slot: slot.clone() });
        (id, slot)
    }

    /// UI answer. `false` means nothing was waiting (stale card, double click,
    /// or the turn already timed out) — the caller keeps its Deny.
    pub fn reply(&self, id: Option<u64>, allow: bool) -> bool {
        let mut inner = self.lock();
        let Some(pending) = inner.pending.as_ref() else {
            return false;
        };
        if let Some(want) = id {
            if want != pending.id {
                return false;
            }
        }
        let slot = pending.slot.clone();
        inner.pending = None;
        let (lock, cv) = &*slot;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = Some(allow);
        cv.notify_all();
        true
    }

    fn clear(&self, id: u64) {
        let mut inner = self.lock();
        if inner.pending.as_ref().map(|p| p.id) == Some(id) {
            inner.pending = None;
        }
    }
}

/// Ask-first gate (spec-s4): emits `tool:approval_required` and waits up to
/// `APPROVAL_WAIT` for the card. No app handle (headless / unit tests) or a
/// timeout ⇒ Deny — silence never becomes consent.
pub struct AskFirstGate {
    broker: Arc<ApprovalBroker>,
    app: Option<tauri::AppHandle>,
}

impl AskFirstGate {
    pub fn new(broker: Arc<ApprovalBroker>, app: Option<tauri::AppHandle>) -> Self {
        AskFirstGate { broker, app }
    }
}

impl ApprovalGate for AskFirstGate {
    fn check(&self, call: &ToolCall, source: &str, remote: bool) -> ApprovalDecision {
        let Some(app) = self.app.as_ref() else {
            log::info!("approval_required(no-ui): tool={} → denied", call.name);
            return ApprovalDecision::Deny;
        };
        let (id, slot) = self.broker.register();
        let args_summary = clip_chars(&mask_secrets(&call.args.to_string()), 200);
        let _ = app.emit(
            "tool:approval_required",
            serde_json::json!({
                "id": id,
                "tool": call.name,
                "source": source,
                "remote": remote,
                "argsSummary": args_summary,
            }),
        );

        let (lock, cv) = &*slot;
        let mut decided = lock.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + APPROVAL_WAIT;
        while decided.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (guard, wait) = cv.wait_timeout(decided, left).unwrap_or_else(|p| p.into_inner());
            decided = guard;
            if wait.timed_out() {
                break;
            }
        }
        let allow = *decided == Some(true);
        drop(decided);
        self.broker.clear(id);
        log::info!("approval_answer: tool={} allow={}", call.name, allow);
        if allow {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny
        }
    }
}

// ---------------------------------------------------------------- commands

fn now_conn<'a>(
    store: &'a tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    store.inner().0.lock().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn mcp_server_list(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<Vec<McpServerRow>, String> {
    let conn = now_conn(&store)?;
    Ok(list_raw(&conn)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| view(r, &conn))
        .collect())
}

/// Save payload (frontend camelCase → snake here).
#[tauri::command]
pub fn mcp_server_save(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: Option<String>,
    name: String,
    transport: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    url: Option<String>,
    headers: Option<Vec<(String, String)>>,
    env: Option<Vec<(String, String)>>,
) -> Result<String, String> {
    let args = args.unwrap_or_default();
    let headers = headers.unwrap_or_default();
    let env = env.unwrap_or_default();
    let conn = now_conn(&store)?;
    match id {
        Some(id) => {
            let existed = row_from(&conn, &id).is_ok();
            if !existed {
                return Err("server_not_found".into());
            }
            update_server(&conn, &id, &name, &transport, command.as_deref(), &args, url.as_deref(), &headers, &env)?;
            agent.manager.remove(&id); // config changed → old connection is stale
            Ok(id)
        }
        None => insert_server(
            &conn,
            &name,
            &transport,
            command.as_deref(),
            &args,
            url.as_deref(),
            &headers,
            &env,
        ),
    }
}

#[tauri::command]
pub fn mcp_server_remove(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    delete_server(&conn, &id)?;
    // Drop the live connection *after* the row is gone, so a failed delete
    // never leaves the pool pointing at nothing.
    agent.manager.remove(&id);
    Ok(())
}

/// Delete one server row; a missing id is an explicit error (never a silent
/// no-op that leaves the UI showing a row that no longer exists).
fn delete_server(conn: &Connection, id: &str) -> Result<(), String> {
    let n = conn
        .execute("DELETE FROM mcp_servers WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("server_not_found".into());
    }
    Ok(())
}

/// Toggle runs a LIVE connect (up to CONNECT_TIMEOUT ≈10s on a dead server)
/// — spawn_blocking per the MAIN-THREAD RULE in dialogue.rs: a sync command
/// would freeze the whole app while the handshake runs.
#[tauri::command]
pub async fn mcp_server_toggle(app: tauri::AppHandle, id: String, enable: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<crate::memory::MemoryStore>();
        let agent = app.state::<AgentState>();
        let conn = now_conn(&store)?;
        if !enable {
            conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id=?1", [&id])
                .map_err(|e| e.to_string())?;
            agent.manager.remove(&id);
            return Ok(());
        }
        let row = row_from(&conn, &id)?;
        let (client, _) = connect_and_cache(&conn, &row)?;
        conn.execute("UPDATE mcp_servers SET enabled=1 WHERE id=?1", [&id])
            .map_err(|e| e.to_string())?;
        agent.manager.insert(id, client);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Connection test: connect + list, never flips `enabled`. Live connect
/// (≈10s timeout) → spawn_blocking (MAIN-THREAD RULE in dialogue.rs).
#[tauri::command]
pub async fn mcp_server_test(app: tauri::AppHandle, id: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<crate::memory::MemoryStore>();
        let conn = now_conn(&store)?;
        let row = row_from(&conn, &id)?;
        let (client, descriptors) = connect_and_cache(&conn, &row)?;
        let count = descriptors.len();
        drop(client); // explicit shutdown — a test must not leave a process behind
        Ok(count)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Cached tool list (5-min TTL semantics: caller may refresh via toggle).
#[tauri::command]
pub fn mcp_server_tools(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: String,
) -> Result<Vec<ToolDescriptor>, String> {
    let conn = now_conn(&store)?;
    let row = row_from(&conn, &id)?;
    row.tool_cache
        .as_ref()
        .and_then(|c| serde_json::from_str::<Vec<ToolDescriptor>>(c).ok())
        .ok_or_else(|| "no_tool_cache".into())
}

/// Built-in tools for the settings page「内置工具」module. Reads the shared
/// registry (agent_tools::builtin_tools), no DB, no connection.
#[tauri::command]
pub fn builtin_tools_list() -> Result<Vec<ToolDescriptor>, String> {
    Ok(crate::agent_tools::builtin_tools())
}

// ---- skills

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub valid: bool,
    pub error_code: Option<&'static str>,
}

#[tauri::command]
pub fn skill_list(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
) -> Result<Vec<SkillView>, String> {
    let conn = now_conn(&store)?;
    let enabled = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten());
    Ok(crate::skill::scan_skills(&agent.skills_dir)
        .into_iter()
        .map(|r| match r {
            Ok(p) => SkillView {
                enabled: enabled.iter().any(|id| id == &p.id),
                valid: true,
                error_code: None,
                id: p.id,
                name: p.name,
                description: p.description,
            },
            Err((id, e)) => {
                let err_code = e.code();
                SkillView {
                    id: id.clone(),
                    name: id,
                    description: String::new(),
                    enabled: false,
                    valid: false,
                    error_code: Some(err_code),
                }
            }
        })
        .collect())
}

#[tauri::command]
pub fn skill_toggle(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
    enable: bool,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    // Only a valid pack can be enabled.
    let packs = crate::skill::scan_skills(&agent.skills_dir);
    if enable && !packs.iter().any(|r| matches!(r, Ok(p) if p.id == id)) {
        return Err("skill_invalid".into());
    }
    let mut ids = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten());
    if enable {
        if !ids.iter().any(|x| x == &id) {
            ids.push(id);
        }
    } else {
        ids.retain(|x| x != &id);
    }
    db::set_setting(&conn, "skill_enabled", &serde_json::to_string(&ids).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_remove(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
) -> Result<(), String> {
    // Guard + delete live in skill.rs so the traversal rule is unit-tested.
    crate::skill::remove_skill_dir(&agent.skills_dir, &id).map_err(|e| e.code().to_string())?;
    let removed = id.clone();
    skill_toggle(store, agent, removed, false)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRefView {
    pub name: String,
    pub content: String,
}

/// Full pack body for the settings-page preview (skill_read). Engine packs
/// are previewable too — they are just packs with a reserved consumer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub refs: Vec<SkillRefView>,
    pub enabled: bool,
    /// body + loaded references, the exact bytes the prompt budget pays for.
    pub size_bytes: usize,
}

#[tauri::command]
pub fn skill_read(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
) -> Result<SkillDetail, String> {
    crate::skill::check_skill_id(&id).map_err(|e| e.code().to_string())?;
    let conn = now_conn(&store)?;
    let enabled = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten())
        .iter()
        .any(|x| x == &id);
    let dir = agent.skills_dir.join(&id);
    let pack = crate::skill::load_skill(&dir).map_err(|e| e.code().to_string())?;
    Ok(SkillDetail {
        size_bytes: pack.body.len() + pack.refs.iter().map(|(_, c)| c.len()).sum::<usize>(),
        refs: pack
            .refs
            .into_iter()
            .map(|(name, content)| SkillRefView { name, content })
            .collect(),
        enabled,
        id: pack.id,
        name: pack.name,
        description: pack.description,
        body: pack.body,
    })
}

/// Import an external pack directory (path from the system folder picker).
/// Returns the imported id; failures carry a mapping key for humanizeAgentError.
#[tauri::command]
pub fn skill_import(
    agent: tauri::State<'_, AgentState>,
    source: String,
) -> Result<String, String> {
    crate::skill::import_skill_dir(&agent.skills_dir, std::path::Path::new(&source))
        .map_err(|e| e.map_key())
}

/// Create a pack from fields typed in the settings UI.
#[tauri::command]
pub fn skill_create(
    agent: tauri::State<'_, AgentState>,
    id: String,
    name: String,
    description: String,
    body: String,
) -> Result<(), String> {
    crate::skill::create_skill_pack(&agent.skills_dir, &id, &name, &description, &body)
        .map_err(|e| e.map_key())
}

/// Load a pack from a remote URL (GitHub repo/tree/raw, GitLab blob, or any
/// https SKILL.md). Network runs on the blocking pool — the fetch is
/// user-initiated and nothing happens until this command is called.
#[tauri::command]
pub async fn skill_load_remote(
    agent: tauri::State<'_, AgentState>,
    url: String,
    id: Option<String>,
) -> Result<String, String> {
    let root = agent.skills_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (derived, files) = crate::skill_remote::fetch(&url)?;
        let id = id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(derived);
        crate::skill::install_pack_files(&root, &id, &files).map_err(|e| e.map_key())?;
        Ok(id)
    })
    .await
    .map_err(|_| "skill_network".to_string())?
}

/// Audit read for the tool-page records card (spec-s6 §3).
#[tauri::command]
pub fn tool_call_records(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    limit: Option<i64>,
) -> Result<Vec<db::ToolCallRecord>, String> {
    let conn = now_conn(&store)?;
    db::tool_call_records(&conn, limit.unwrap_or(20).clamp(1, 200)).map_err(|e| e.to_string())
}

/// 计划历史（T6 / plan-planning §5.6）：工具页进度面板首屏基线。
/// 复用 `db::task_list`（含解析后的 plan_json），不另写一份状态逻辑；
/// 默认 10 条、上限 50——面板只看最近几轮，不做无限历史。
#[tauri::command]
pub fn plan_list(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    limit: Option<i64>,
) -> Result<Vec<db::TaskRow>, String> {
    let conn = now_conn(&store)?;
    db::task_list(&conn, limit.unwrap_or(10).clamp(1, 50)).map_err(|e| e.to_string())
}

/// Answer for the approval card (spec-s4/s6). Returns whether a turn was
/// actually waiting — a stale card (timeout already fired) gets `false` and
/// changes nothing. Deliberately touches no DB: the chat turn holds the store
/// mutex while it waits for exactly this reply.
#[tauri::command]
pub fn mcp_approval_reply(
    agent: tauri::State<'_, AgentState>,
    id: Option<u64>,
    allow: bool,
) -> Result<bool, String> {
    Ok(agent.approvals.reply(id, allow))
}

/// Current tool-approval mode: `auto` (default) or `ask` (spec-s4 §1).
#[tauri::command]
pub fn agent_approval_get(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<String, String> {
    let conn = now_conn(&store)?;
    Ok(match db::get_setting(&conn, "approval_mode").ok().flatten().as_deref() {
        Some("ask") => "ask",
        _ => "auto",
    }
    .to_string())
}

/// Flip the gate. Only the two documented values are accepted — anything else
/// would silently fall back to `auto`, which is the wrong direction to guess.
#[tauri::command]
pub fn agent_approval_set(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    mode: String,
) -> Result<(), String> {
    if mode != "auto" && mode != "ask" {
        return Err("bad_approval_mode".into());
    }
    let conn = now_conn(&store)?;
    db::set_setting(&conn, "approval_mode", &mode).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        db::open(&{
            let dir = std::env::temp_dir().join("bonsai-mcp-admin-test");
            std::fs::create_dir_all(&dir).unwrap();
            dir.join(format!("t-{}-{}.sqlite3", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)))
        })
        .unwrap()
    }

    /// Store-backed twin for tests that go through `execute_audited` (it
    /// locks the store internally — 2026-11-20 lock-narrowing).
    fn mem_store() -> crate::memory::MemoryStore {
        let dir = std::env::temp_dir().join("bonsai-mcp-admin-test");
        std::fs::create_dir_all(&dir).unwrap();
        crate::memory::MemoryStore::open(&dir.join(format!(
            "s-{}-{}.sqlite3",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        )))
        .unwrap()
    }

    fn with_conn<T>(s: &crate::memory::MemoryStore, f: impl FnOnce(&Connection) -> T) -> T {
        let conn = s.0.lock().unwrap();
        f(&conn)
    }

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // cross-module helpers (mcp_discover tests share the temp-db pattern)
    pub(crate) fn mem_conn() -> Connection {
        mem()
    }

    pub(crate) fn next_seq() -> u32 {
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn view_for_test(row: &RawRow, conn: &Connection) -> McpServerRow {
        view(row, conn)
    }

    #[test]
    fn save_list_masks_secrets() {
        let conn = mem();
        let id = insert_server(
            &conn,
            "远程天气",
            "http",
            None,
            &[],
            Some("https://mcp.example.com/v1"),
            &hdrs(&[("Authorization", "Bearer supersecret99")]),
            &[],
        )
        .unwrap();
        let rows = list_raw(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        let v = view(&rows[0], &conn);
        assert!(!v.enabled, "new server starts disabled (RK-04)");
        let rendered = serde_json::to_string(&v).unwrap();
        assert!(!rendered.contains("supersecret99"), "plaintext must not leak");
        assert!(rendered.contains("••••"));
        assert_eq!(v.id, id);
    }

    #[test]
    fn save_validation_rejects_bad_transport_fields() {
        let conn = mem();
        assert_eq!(
            insert_server(&conn, "x", "stdio", None, &[], None, &[], &[]).unwrap_err(),
            "bad_command"
        );
        assert_eq!(
            insert_server(&conn, "x", "http", None, &[], Some("ftp://x"), &[], &[]).unwrap_err(),
            "bad_url"
        );
        assert_eq!(
            insert_server(&conn, "x", "http", None, &[], None, &[], &[]).unwrap_err(),
            "bad_url"
        );
    }

    #[test]
    fn remove_deletes_row_and_reports_missing() {
        let conn = mem();
        let id = insert_server(&conn, "n", "http", None, &[], Some("https://x.com"), &[], &[]).unwrap();
        delete_server(&conn, &id).unwrap();
        assert!(list_raw(&conn).unwrap().is_empty());
        assert_eq!(delete_server(&conn, &id).unwrap_err(), "server_not_found");
    }

    #[test]
    fn update_missing_row_errors_loudly() {
        let conn = mem();
        assert_eq!(
            update_server(&conn, "nope", "n", "http", None, &[], Some("https://x.com"), &[], &[]).unwrap_err(),
            "server_not_found"
        );
    }

    #[test]
    fn toggle_flow_with_mock_client() {
        // No live client in CI: connect fails → enabled must stay 0.
        let conn = mem();
        let id = insert_server(
            &conn,
            "本地",
            "stdio",
            Some("/nonexistent-bonsai-mcp-bin"),
            &[],
            None,
            &[],
            &[],
        )
        .unwrap();
        let row = row_from(&conn, &id).unwrap();
        assert!(connect_and_cache(&conn, &row).is_err());
        let enabled: i64 = conn
            .query_row("SELECT enabled FROM mcp_servers WHERE id=?1", [&id], |r| r.get(0))
            .unwrap();
        assert_eq!(enabled, 0, "failed connect must not flip enabled");
    }

    #[test]
    fn audit_rows_round_trip_masked() {
        let conn = mem();
        // The caller (execute_audited) masks before logging — same here.
        let summary = mask_secrets("args {\"auth\": \"Bearer tokensecret1\"}");
        db::log_agent_tool_call(
            &conn,
            "mcp",
            "weather",
            "ok",
            "auto_allowed",
            Some(&summary),
            Some("trace-1"),
            true,
            Some(42),
            None,
        )
        .unwrap();
        let records = db::tool_call_records(&conn, 10).unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert!(r.remote);
        assert_eq!(r.result_status.as_deref(), Some("auto_allowed"));
        assert!(r.params_summary.as_deref().unwrap_or("").contains("••••"));
        assert!(!r.params_summary.as_deref().unwrap_or("").contains("tokensecret1"));
    }

    #[test]
    fn auto_gate_allows_ask_gate_denies() {
        use crate::agent_tools::AutoAllowGate;
        let call = ToolCall { name: "t".into(), args: serde_json::json!({}) };
        assert_eq!(AutoAllowGate.check(&call, "s", false), ApprovalDecision::Allow);
        // No app handle → no one can answer → Deny (never a silent allow).
        let headless = AskFirstGate::new(Arc::new(ApprovalBroker::default()), None);
        assert_eq!(headless.check(&call, "s", false), ApprovalDecision::Deny);
    }

    #[test]
    fn approval_broker_handshake() {
        let broker = Arc::new(ApprovalBroker::default());
        assert!(!broker.reply(Some(1), true), "nothing pending → stale reply");

        let (id, slot) = broker.register();
        assert!(!broker.reply(Some(id + 99), true), "id mismatch is ignored");
        assert!(
            broker.lock().pending.is_some(),
            "mismatched reply must not consume the slot"
        );
        assert!(broker.reply(Some(id), true));
        let (lock, _) = &*slot;
        assert_eq!(*lock.lock().unwrap(), Some(true));
        assert!(!broker.reply(Some(id), true), "second reply is a no-op");
    }

    #[test]
    fn execute_audited_unknown_tool_errors() {
        let s = mem_store();
        let kit = AgentKit::empty();
        let err = execute_audited(&s, &kit, "ghost", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "unknown_tool");
    }

    #[test]
    fn execute_audited_records_ok_and_failure_status() {
        let s = mem_store();

        // ok path: auto_allowed + masked params.
        let mut ok = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        ok.replies.insert("weather".into(), "杭州多云 18 度".into());
        let kit = AgentKit {
            providers: vec![Box::new(ok)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        kit.trace("round1:candidates=1".into());
        let out = execute_audited(
            &s,
            &kit,
            "weather",
            serde_json::json!({"auth": "Bearer sk-abcdefgh123"}),
        )
        .unwrap();
        assert_eq!(out.summary, "杭州多云 18 度");

        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
        assert_eq!(rec.tool, "mcp");
        assert_eq!(rec.action, "weather");
        assert_eq!(rec.outcome, "ok");
        assert_eq!(rec.result_status.as_deref(), Some("auto_allowed"));
        assert!(rec.duration_ms.is_some());
        assert_eq!(rec.prompt_trace.as_deref(), Some("round1:candidates=1"));
        let params = rec.params_summary.as_deref().unwrap_or("");
        assert!(params.contains("••••"), "params masked: {params}");
        assert!(!params.contains("abcdefgh123"));

        // failure path: execute Err → outcome=error + specific result_status.
        let mut broken = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        broken.fail_for.push("weather".into());
        let kit_broken = AgentKit {
            providers: vec![Box::new(broken)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        let err = execute_audited(&s, &kit_broken, "weather", serde_json::json!({})).unwrap_err();
        assert_eq!(err, crate::mcp::ERR_TIMEOUT);

        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
        assert_eq!(rec.outcome, "error");
        assert_eq!(rec.result_status.as_deref(), Some("mcp_timeout"));
    }

    #[test]
    fn execute_audited_validates_args_before_gate() {
        // plan §3.3: strict inputSchema check BEFORE the approval gate —
        // a small model's hallucinated args never reach execution, and the
        // audit row records the exact reason.
        let s = mem_store();
        let schema = r#"{"type":"object","properties":{"city":{"type":"string"},"days":{"type":"integer"}},"required":["city"]}"#;
        let mut provider = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", schema);
        provider.replies.insert("weather".into(), "杭州多云 18 度".into());
        let kit = AgentKit {
            providers: vec![Box::new(provider)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };

        // missing required → rejected, provider never called.
        let err =
            execute_audited(&s, &kit, "weather", serde_json::json!({"days": 3})).unwrap_err();
        assert_eq!(err, "bad_args:missing:city");

        // wrong type → rejected.
        let err = execute_audited(&s, &kit, "weather", serde_json::json!({"city": "杭州", "days": "三天"}))
            .unwrap_err();
        assert_eq!(err, "bad_args:type:days");

        // not an object → rejected.
        let err = execute_audited(&s, &kit, "weather", serde_json::json!("杭州")).unwrap_err();
        assert_eq!(err, "bad_args:not-an-object");

        // valid args (incl. unknown extra field) → passes, executes.
        let out = execute_audited(&s, &kit, "weather", serde_json::json!({"city": "杭州", "unit": "c"})).unwrap();
        assert_eq!(out.summary, "杭州多云 18 度");

        // audit: only the rejected rows carry bad_args; the good one auto_allowed.
        let recs = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        let bad = recs.iter().filter(|r| r.result_status.as_deref().unwrap_or("").starts_with("bad_args"));
        assert_eq!(bad.count(), 3);
        let good = &recs[0];
        assert_eq!(good.result_status.as_deref(), Some("auto_allowed"));
        assert_eq!(good.outcome, "ok");
    }

    /// Startup reconnect (2026-11-20 getHardwareInfo regression): disabled
    /// rows must be skipped, an enabled-but-dead server must fail CLEANLY
    /// (logged, pool stays empty, count reflects reality).
    #[test]
    fn reconnect_enabled_servers_skips_dead_and_disabled() {
        let dir = std::env::temp_dir().join("bonsai-mcp-reconnect-test");
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::memory::MemoryStore::open(
            &dir.join(format!("r-{}-{}.sqlite3", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst))),
        )
        .unwrap();
        let agent = AgentState {
            manager: crate::mcp::McpManager::default(),
            skills_dir: std::env::temp_dir(),
            approvals: Arc::new(ApprovalBroker::default()),
            app: None,
        };
        let conn = store.0.lock().unwrap();
        let id = insert_server(&conn, "死掉的", "stdio", Some("/bin/true"), &[], None, &[], &[]).unwrap();
        drop(conn);

        // Disabled row: skipped without any connection attempt.
        assert_eq!(reconnect_enabled_servers(&agent, &store), 0);
        assert!(agent.manager.get(&id).is_none(), "disabled row must not enter the pool");

        // Enabled but not a real MCP server (/bin/true exits immediately →
        // initialize fails): clean skip, pool stays empty.
        {
            let conn = store.0.lock().unwrap();
            conn.execute("UPDATE mcp_servers SET enabled=1 WHERE id=?1", [&id]).unwrap();
        }
        assert_eq!(reconnect_enabled_servers(&agent, &store), 0);
        assert!(agent.manager.get(&id).is_none(), "dead server must not enter the pool");
    }

    #[test]
    fn ask_mode_gate_denies_when_no_ui_is_attached() {
        // Full flip check: setting=ask → build_agent_kit picks the asking
        // gate → without an app handle it must deny (never silently allow).
        let s = mem_store();
        with_conn(&s, |conn| {
            db::set_setting(conn, "approval_mode", "ask").unwrap();
            let id = insert_server(conn, "本地", "stdio", Some("/bin/true"), &[], None, &[], &[]).unwrap();
            let cache = serde_json::to_string(&vec![ToolDescriptor {
                name: "weather".into(),
                summary: "查天气".into(),
                detail: "{}".into(),
                kind: crate::agent_tools::ToolKind::Mcp,
                remote: false,
                        access: crate::agent_tools::ToolAccess::Read,
            }])
            .unwrap();
            conn.execute(
                "UPDATE mcp_servers SET enabled = 1, tool_cache = ?1 WHERE id = ?2",
                rusqlite::params![cache, id],
            )
            .unwrap();
        });
        let state = AgentState {
            manager: crate::mcp::McpManager::default(),
            skills_dir: std::env::temp_dir(),
            approvals: Arc::new(ApprovalBroker::default()),
            app: None,
        };
        // Separate scopes: execute_audited locks internally, so no guard may
        // be alive here.
        let kit = with_conn(&s, |conn| build_agent_kit(conn, &state, "今天有什么会"));
        assert!(kit.has_capabilities());
        let err = execute_audited(&s, &kit, "weather", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "denied");
        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
        assert_eq!(rec.outcome, "denied");
        assert_eq!(rec.result_status.as_deref(), Some("denied_by_gate"));
    }

    #[test]
    fn catalog_is_sorted_and_capped() {
        // Catalog order must be stable regardless of provider/enable order:
        // the model picks names from this list (spec-s1 §3 item 2).
        let kit = AgentKit {
            providers: vec![
                Box::new(crate::mcp::MockMcpProvider::with_tool("zebra", "z", "{}")),
                Box::new(crate::mcp::MockMcpProvider::with_tool("alpha", "a", "{}")),
            ],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        let names: Vec<String> = kit.catalog().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["alpha".to_string(), "zebra".to_string()]);
        assert!(kit.has_capabilities());
        assert!(!AgentKit::empty().has_capabilities());
    }

    // ---- 权限三档（plan-task-card v0.2 §6，roadmap G-10）

    /// Always-allow gate, for the ask-branch allow-side assertion.
    struct AllowAllGate;
    impl ApprovalGate for AllowAllGate {
        fn check(&self, _c: &ToolCall, _s: &str, _r: bool) -> ApprovalDecision {
            ApprovalDecision::Allow
        }
    }

    fn kit_with_access(name: &str, access: crate::agent_tools::ToolAccess) -> AgentKit {
        let mut p = crate::mcp::MockMcpProvider::with_tool(name, "测试工具", "{}");
        p.replies.insert(name.to_string(), "执行结果".into());
        let mut d = p.tools[0].clone();
        d.access = access;
        p.tools[0] = d;
        AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        }
    }

    #[test]
    fn forbidden_tool_never_executes_and_audits_rejection() {
        let s = crate::memory::MemoryStore::open(&std::env::temp_dir().join("mcp-admin-t3a.sqlite3")).unwrap();
        let kit = kit_with_access("rm_rf", crate::agent_tools::ToolAccess::Forbidden);
        let err = execute_audited(&s, &kit, "rm_rf", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "forbidden");
        // 沉默不得变成执行：审计记录拒绝，状态 forbidden_tool。
        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
        assert_eq!(rec.outcome, "denied");
        assert_eq!(rec.result_status.as_deref(), Some("forbidden_tool"));
    }

    #[test]
    fn ask_access_forces_the_ask_gate_even_in_auto_mode() {
        let s = crate::memory::MemoryStore::open(&std::env::temp_dir().join("mcp-admin-t3b.sqlite3")).unwrap();
        // kit.gate 是 AutoAllowGate（approval_mode=auto 的形状），但 ask 档
        // 工具必须走 ask_gate——headless 无 AppHandle ⇒ 立即拒绝。
        let kit = kit_with_access("send_mail", crate::agent_tools::ToolAccess::Ask);
        let err = execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "denied");
        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())[0];
        assert_eq!(rec.result_status.as_deref(), Some("denied_by_gate"));

        // ask 门放行（用户点了允许）→ 执行 + asked_allowed 审计。
        let mut p = crate::mcp::MockMcpProvider::with_tool("send_mail", "测试工具", "{}");
        p.replies.insert("send_mail".into(), "已执行".into());
        let mut d = p.tools[0].clone();
        d.access = crate::agent_tools::ToolAccess::Ask;
        p.tools[0] = d;
        let kit = AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AllowAllGate),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        let out = execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap();
        assert_eq!(out.summary, "已执行");
        let rec = &with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap())
            .into_iter()
            .find(|r| r.result_status.as_deref() == Some("asked_allowed"))
            .expect("asked_allowed audit row");
        assert_eq!(rec.outcome, "ok");
    }

    #[test]
    fn access_defaults_to_read_for_legacy_caches() {
        // 旧 tool_cache 反序列化（无 access 字段）→ Read，行为零回归。
        let old: crate::agent_tools::ToolDescriptor = serde_json::from_str(
            r#"{"name":"weather","summary":"查天气","detail":"{}","kind":"mcp","remote":false}"#,
        )
        .unwrap();
        assert_eq!(old.access, crate::agent_tools::ToolAccess::Read);
        assert_eq!(old.access.as_str(), "read");
    }

    // ---- 轮内工具缓存（plan-tool-cache v2）

    #[test]
    fn execute_audited_memoizes_read_tools_within_one_kit() {
        let s = mem_store();
        let mut p = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        p.replies.insert("weather".into(), "杭州多云 18 度".into());
        let kit = AgentKit {
            providers: vec![Box::new(p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            ask_gate: Box::new(AskFirstGate::new(std::sync::Arc::new(ApprovalBroker::default()), None)),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        let args = serde_json::json!({"city": "杭州", "unit": "c"});
        let out1 = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
        // 键序不同的同参数对象 → 同一缓存键 → 命中，provider 不再执行。
        let out2 = execute_audited(&s, &kit, "weather", serde_json::json!({"unit": "c", "city": "杭州"})).unwrap();
        assert_eq!(out1.summary, out2.summary);

        // 审计诚实：一次真执行 + 一次 cache_hit（新行在前）。
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records[0].result_status.as_deref(), Some("cache_hit"));
        assert_eq!(records[0].outcome, "ok");
        assert_eq!(records[1].result_status.as_deref(), Some("auto_allowed"));
    }

    #[test]
    fn write_tool_success_clears_the_turn_cache() {
        let s = mem_store();
        let mut read_p = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        read_p.replies.insert("weather".into(), "执行结果".into());
        let mut write_p = crate::mcp::MockMcpProvider::with_tool("send_mail", "发邮件", "{}");
        write_p.replies.insert("send_mail".into(), "已执行".into());
        let mut d = write_p.tools[0].clone();
        d.access = crate::agent_tools::ToolAccess::Ask;
        write_p.tools[0] = d;
        let kit = AgentKit {
            providers: vec![Box::new(read_p), Box::new(write_p)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            // ask 门放行（模拟用户点了允许）→ 写成功 → 必须清缓存。
            ask_gate: Box::new(AllowAllGate),
            task_id: Mutex::new(None),            prompt_trace: Mutex::new(Vec::new()),            tool_cache: Default::default(),
            ..AgentKit::empty()
        };
        let args = serde_json::json!({"city": "杭州"});
        execute_audited(&s, &kit, "weather", args.clone()).unwrap();
        // 命中一次，证明缓存确实在工作。
        let hit = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
        assert_eq!(hit.summary, "执行结果");
        // 写成功 → 清空 → 第三次 weather 重新真执行（非 cache_hit）。
        execute_audited(&s, &kit, "send_mail", serde_json::json!({})).unwrap();
        let out3 = execute_audited(&s, &kit, "weather", args.clone()).unwrap();
        assert_eq!(out3.summary, "执行结果");
        let records = with_conn(&s, |conn| db::tool_call_records(conn, 10).unwrap());
        assert_eq!(records[0].result_status.as_deref(), Some("auto_allowed"));
        assert_eq!(records[0].action, "weather");
        assert_eq!(records[1].result_status.as_deref(), Some("asked_allowed"));
        assert_eq!(records[2].result_status.as_deref(), Some("cache_hit"));
    }
}
