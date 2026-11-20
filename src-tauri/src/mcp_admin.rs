//! MCP server administration + the agent kit assembly (spec-s2/s4).
//!
//! SQL for `mcp_servers` lives here (agent-owned table); `tool_calls`
//! primitives stay in memory/db.rs. Everything the frontend receives has
//! secrets masked — plaintext headers/env only ever exist inside the DB
//! and the live transport construction.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use tauri::Emitter;

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

fn list_raw(conn: &Connection) -> rusqlite::Result<Vec<RawRow>> {
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

fn insert_server(
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
    conn.execute(
        "UPDATE mcp_servers SET tool_cache=?2, tool_cache_at=?3 WHERE id=?1",
        rusqlite::params![
            row.id,
            serde_json::to_string(&descriptors).unwrap_or_default(),
            crate::memory::clock::now_iso()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok((client, descriptors))
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
    pub prompt_trace: Mutex<Vec<String>>,
}

impl AgentKit {
    pub fn empty() -> Self {
        AgentKit {
            providers: Vec::new(),
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            prompt_trace: Mutex::new(Vec::new()),
        }
    }

    pub fn has_capabilities(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Flat descriptor catalog (≤20, stable order) for the level-0 prompt.
    pub fn catalog(&self) -> Vec<ToolDescriptor> {
        let mut all = Vec::new();
        for p in &self.providers {
            all.extend(p.descriptors());
        }
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all.truncate(20);
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

/// Execute a validated call through the gate + provider, fully audited.
pub fn execute_audited(
    conn: &Connection,
    kit: &AgentKit,
    name: &str,
    args: serde_json::Value,
) -> Result<ToolOutput, String> {
    let (_idx, provider, descriptor) =
        find_owner(kit, name).ok_or_else(|| "unknown_tool".to_string())?;
    let args_summary = crate::agent_tools::clip_chars(&mask_secrets(&args.to_string()), 200);
    let started = std::time::Instant::now();
    let call = ToolCall { name: name.to_string(), args };
    let decision = kit.gate.check(&call, &descriptor.name, descriptor.remote);
    if decision == ApprovalDecision::Deny {
        db::log_agent_tool_call(
            conn,
            descriptor.kind.as_str(),
            &descriptor.name,
            "denied",
            "denied_by_gate",
            Some(&args_summary),
            Some(&kit.trace_ref()),
            descriptor.remote,
            Some(started.elapsed().as_millis() as i64),
        )
        .ok();
        return Err("denied".into());
    }
    let result = provider.execute(&call);
    let duration = Some(started.elapsed().as_millis() as i64);
    match result {
        Ok(out) => {
            db::log_agent_tool_call(
                conn,
                descriptor.kind.as_str(),
                &descriptor.name,
                "ok",
                if descriptor.remote { "auto_allowed_remote" } else { "auto_allowed" },
                Some(&args_summary),
                Some(&kit.trace_ref()),
                descriptor.remote,
                duration,
            )
            .ok();
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
            db::log_agent_tool_call(
                conn,
                descriptor.kind.as_str(),
                &descriptor.name,
                if status == "mcp_unreachable" { "unavailable" } else { "error" },
                status,
                Some(&args_summary),
                Some(&kit.trace_ref()),
                descriptor.remote,
                duration,
            )
            .ok();
            Err(e)
        }
    }
}

/// Build the turn's kit from managed state + settings. Cost: one SQL read
/// per turn (settings + enabled servers); live clients come from the pool.
pub fn build_agent_kit(conn: &Connection, state: &AgentState) -> AgentKit {
    let mut providers: Vec<Box<dyn ToolProvider>> = Vec::new();

    // Skills first (prompt-only), then MCP tools — catalog order is stable
    // either way (sorted by name in AgentKit::catalog).
    let enabled = crate::skill::enabled_ids(db::get_setting(conn, "skill_enabled").ok().flatten());
    if !enabled.is_empty() {
        let packs: Vec<crate::skill::SkillPack> = crate::skill::scan_skills(&state.skills_dir)
            .into_iter()
            .filter_map(|r| r.ok())
            .filter(|p| enabled.iter().any(|id| id == &p.id))
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

    AgentKit { providers, gate, prompt_trace: Mutex::new(Vec::new()) }
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

#[tauri::command]
pub fn mcp_server_toggle(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
    enable: bool,
) -> Result<(), String> {
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
}

/// Connection test: connect + list, never flips `enabled`.
#[tauri::command]
pub fn mcp_server_test(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: String,
) -> Result<usize, String> {
    let conn = now_conn(&store)?;
    let row = row_from(&conn, &id)?;
    let (client, descriptors) = connect_and_cache(&conn, &row)?;
    let count = descriptors.len();
    drop(client); // explicit shutdown — a test must not leave a process behind
    Ok(count)
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

/// Audit read for the tool-page records card (spec-s6 §3).
#[tauri::command]
pub fn tool_call_records(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    limit: Option<i64>,
) -> Result<Vec<db::ToolCallRecord>, String> {
    let conn = now_conn(&store)?;
    db::tool_call_records(&conn, limit.unwrap_or(20).clamp(1, 200)).map_err(|e| e.to_string())
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
mod tests {
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

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
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
        let conn = mem();
        let kit = AgentKit::empty();
        let err = execute_audited(&conn, &kit, "ghost", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "unknown_tool");
    }

    #[test]
    fn execute_audited_records_ok_and_failure_status() {
        let conn = mem();

        // ok path: auto_allowed + masked params.
        let mut ok = crate::mcp::MockMcpProvider::with_tool("weather", "查天气", "{}");
        ok.replies.insert("weather".into(), "杭州多云 18 度".into());
        let kit = AgentKit {
            providers: vec![Box::new(ok)],
            gate: Box::new(crate::agent_tools::AutoAllowGate),
            prompt_trace: Mutex::new(Vec::new()),
        };
        kit.trace("round1:candidates=1".into());
        let out = execute_audited(
            &conn,
            &kit,
            "weather",
            serde_json::json!({"auth": "Bearer sk-abcdefgh123"}),
        )
        .unwrap();
        assert_eq!(out.summary, "杭州多云 18 度");

        let rec = &db::tool_call_records(&conn, 10).unwrap()[0];
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
            prompt_trace: Mutex::new(Vec::new()),
        };
        let err = execute_audited(&conn, &kit_broken, "weather", serde_json::json!({})).unwrap_err();
        assert_eq!(err, crate::mcp::ERR_TIMEOUT);

        let rec = &db::tool_call_records(&conn, 10).unwrap()[0];
        assert_eq!(rec.outcome, "error");
        assert_eq!(rec.result_status.as_deref(), Some("mcp_timeout"));
    }

    #[test]
    fn ask_mode_gate_denies_when_no_ui_is_attached() {
        // Full flip check: setting=ask → build_agent_kit picks the asking
        // gate → without an app handle it must deny (never silently allow).
        let conn = mem();
        db::set_setting(&conn, "approval_mode", "ask").unwrap();
        let state = AgentState {
            manager: crate::mcp::McpManager::default(),
            skills_dir: std::env::temp_dir(),
            approvals: Arc::new(ApprovalBroker::default()),
            app: None,
        };
        let id = insert_server(&conn, "本地", "stdio", Some("/bin/true"), &[], None, &[], &[]).unwrap();
        let cache = serde_json::to_string(&vec![ToolDescriptor {
            name: "weather".into(),
            summary: "查天气".into(),
            detail: "{}".into(),
            kind: crate::agent_tools::ToolKind::Mcp,
            remote: false,
        }])
        .unwrap();
        conn.execute(
            "UPDATE mcp_servers SET enabled = 1, tool_cache = ?1 WHERE id = ?2",
            rusqlite::params![cache, id],
        )
        .unwrap();

        let kit = build_agent_kit(&conn, &state);
        assert!(kit.has_capabilities());
        let err = execute_audited(&conn, &kit, "weather", serde_json::json!({})).unwrap_err();
        assert_eq!(err, "denied");
        let rec = &db::tool_call_records(&conn, 10).unwrap()[0];
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
            prompt_trace: Mutex::new(Vec::new()),
        };
        let names: Vec<String> = kit.catalog().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["alpha".to_string(), "zebra".to_string()]);
        assert!(kit.has_capabilities());
        assert!(!AgentKit::empty().has_capabilities());
    }
}
