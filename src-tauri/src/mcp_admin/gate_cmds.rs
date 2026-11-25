//! Audit / plan / task resume / approval-mode / budget commands.

use crate::memory::db;

use super::state::AgentState;
use super::now_conn;

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

/// Phase 3 A1：显式准备恢复（D1：不自动执工具；写步骤可能 ask_user）。
#[tauri::command]
pub fn task_resume(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    task_id: String,
) -> Result<crate::task_recovery::ResumePrepareResult, String> {
    let conn = now_conn(&store)?;
    crate::task_recovery::prepare_resume(&conn, &task_id).map_err(|e| e.to_string())
}

/// Phase 3 A2：用户对 ask_user 步骤的选择（retry|skip|cancel）。
#[tauri::command]
pub fn task_reentry_choice(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    task_id: String,
    choice: String,
) -> Result<crate::task_recovery::ResumePrepareResult, String> {
    let conn = now_conn(&store)?;
    crate::task_recovery::apply_reentry_choice(&conn, &task_id, &choice).map_err(|e| e.to_string())
}

/// Phase 3 A1：可恢复任务列表（启动/面板）。
#[tauri::command]
pub fn task_list_resumable(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    limit: Option<i64>,
) -> Result<Vec<crate::task_recovery::ResumableTask>, String> {
    let conn = now_conn(&store)?;
    crate::task_recovery::list_resumable(&conn, limit.unwrap_or(10)).map_err(|e| e.to_string())
}

/// Answer for the approval card (spec-s4/s6). Returns whether a turn was
/// actually waiting — a stale card (timeout already fired) gets `false` and
/// changes nothing. Deliberately touches no DB: the chat turn holds the store
/// mutex while it waits for exactly this reply.
///
/// `scope`: `"once"`（默认）只放行本次；`"session"` 同时把该工具记入本会话
/// 白名单，后续同会话同工具不再弹卡（Forbidden 仍在 execute_audited 拦）。
#[tauri::command]
pub fn mcp_approval_reply(
    agent: tauri::State<'_, AgentState>,
    id: Option<u64>,
    allow: bool,
    scope: Option<String>,
) -> Result<bool, String> {
    let tool = agent.approvals.peek_tool(id);
    let ok = agent.approvals.reply(id, allow);
    if ok && allow && scope.as_deref() == Some("session") {
        if let Some(name) = tool {
            agent.session_allow.grant(&name);
        }
    }
    Ok(ok)
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

/// 云端循环预算开关（plan-chat-agent-inline）：默认均关；上限数字固定 32。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBudgetCaps {
    pub tool_cap_enabled: bool,
    pub request_cap_enabled: bool,
    pub tool_cap: usize,
    pub request_cap: usize,
}

#[tauri::command]
pub fn agent_budget_get(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<AgentBudgetCaps, String> {
    let conn = now_conn(&store)?;
    let tool = db::get_setting(&conn, crate::dialogue::SETTING_TOOL_CAP)
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    let request = db::get_setting(&conn, crate::dialogue::SETTING_REQUEST_CAP)
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    Ok(AgentBudgetCaps {
        tool_cap_enabled: tool,
        request_cap_enabled: request,
        tool_cap: crate::dialogue::CLOUD_TOOL_CALL_CAP,
        request_cap: crate::dialogue::CLOUD_REQUEST_CAP,
    })
}

#[tauri::command]
pub fn agent_budget_set(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    tool_cap_enabled: bool,
    request_cap_enabled: bool,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    db::set_setting(
        &conn,
        crate::dialogue::SETTING_TOOL_CAP,
        if tool_cap_enabled { "true" } else { "false" },
    )
    .map_err(|e| e.to_string())?;
    db::set_setting(
        &conn,
        crate::dialogue::SETTING_REQUEST_CAP,
        if request_cap_enabled { "true" } else { "false" },
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
