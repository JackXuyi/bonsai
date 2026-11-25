//! MCP server administration + the agent kit assembly (spec-s2/s4).
//!
//! SQL for `mcp_servers` lives in [`servers`]; `tool_calls` primitives stay
//! in memory/db.rs. Everything the frontend receives has secrets masked —
//! plaintext headers/env only ever exist inside the DB and the live
//! transport construction.
//!
//! File split (2026-11-25, AGENTS §12): state / servers / approval / runtime /
//! skill_cmds / gate_cmds — behavior unchanged; `crate::mcp_admin::*` re-exports
//! keep stable for non-command callers. Tauri commands are addressed via their
//! public submodules in `generate_handler!` (same pattern as `dialogue::commands`).

use std::sync::MutexGuard;

use rusqlite::Connection;

mod state;
pub mod servers;
pub mod approval;
pub mod runtime;
pub mod skill_cmds;
pub mod gate_cmds;

#[cfg(test)]
pub(crate) mod tests;

pub use state::{AgentState, SessionToolAllow};
pub use approval::{ApprovalBroker, AskFirstGate};
pub use runtime::{
    build_agent_kit, dialogue_cancel, execute_audited, register_cancel, unregister_cancel,
    AgentKit,
};
pub use servers::{new_task_id, reconnect_enabled_servers, McpServerRow};
pub(crate) use servers::{insert_server, list_raw, row_signature, uuid_v4, RawRow}; // RawRow: mcp_discover tests

/// How long cached MCP tool lists are considered fresh (calendar_cache 惯例).
pub const TOOL_CACHE_TTL_SECS: i64 = 300;
/// How long `ask` mode waits for the user before treating silence as denial.
/// 授权等待（plan-chat-agent-inline v1.2）：默认 1 分钟；超时视为拒绝。
pub const APPROVAL_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn now_conn<'a>(
    store: &'a tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<MutexGuard<'a, Connection>, String> {
    store.inner().write_guard().map_err(|e| e.to_string())
}

#[cfg(test)]
pub use runtime::cancel_requested;
