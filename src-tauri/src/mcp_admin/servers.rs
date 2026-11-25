//! MCP server rows, SQL, reconnect, and Tauri admin commands.

use std::sync::Arc;

use rusqlite::Connection;
use serde::Serialize;
use tauri::Manager;

use crate::agent_tools::{mask_secrets, ToolDescriptor};
use crate::mcp::{McpClient, McpTransport};

use super::state::AgentState;
use super::now_conn;

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

pub(crate) fn row_from(conn: &Connection, id: &str) -> Result<RawRow, String> {
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

pub(crate) fn view(row: &RawRow, conn: &Connection) -> McpServerRow {
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

pub(crate) fn update_server(conn: &Connection, id: &str, name: &str, transport: &str, command: Option<&str>, args: &[String], url: Option<&str>, headers: &[(String, String)], env: &[(String, String)]) -> Result<(), String> {
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
pub(crate) fn uuid_v4() -> String {
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

pub(crate) static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
pub(crate) fn connect_and_cache(conn: &Connection, row: &RawRow) -> Result<(Arc<McpClient>, Vec<ToolDescriptor>), String> {
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
        let conn = match store.write_guard() {
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
            let conn = match store.write_guard() {
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

// ---------------------------------------------------------------- commands

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
pub(crate) fn delete_server(conn: &Connection, id: &str) -> Result<(), String> {
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
