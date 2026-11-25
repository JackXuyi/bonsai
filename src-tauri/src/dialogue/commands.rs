//! Tauri commands for dialogue / history / trace.

use chrono::Timelike;
use tauri::Manager;

use crate::memory::db;
use crate::memory::MemoryStore;
use crate::persona::{Persona, PersonaState};

use super::proactive::proactive_text;
use super::turn::{chat_turn_streaming_with_kit, chat_turn_with_kit};
use super::types::{DialogueOutcome, ProactiveReply};

//
// MAIN-THREAD RULE (2026-11-20 freeze regression): every chat command runs
// its FULL pipeline — up to 3 cloud-model rounds (deepseek ≈8-12s each), the
// memory LIKE scan, and MCP tool execution (disk_report ≈13s) — inside
// `spawn_blocking`. Sync `#[tauri::command]` fns execute on the MAIN thread
// in Tauri 2, which froze the window/animation for 20-45s per message.
// Commands stay `async fn` with a 'static closure: states are fetched via
// `app.state::<T>()` inside the closure (the borrow lives only within it).
// Deliberately NOT moved: calendar commands — EventKit must stay on the
// main thread (TCC ownership, calendar.rs).

#[tauri::command]
pub async fn dialogue_chat(
    app: tauri::AppHandle,
    user_text: String,
    session_id: Option<String>,
    raining: Option<bool>,
) -> Result<DialogueOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<MemoryStore>();
        let card: Persona = app
            .state::<PersonaState>()
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        let calendar = app.state::<crate::calendar::CalendarState>();
        let agent = app.state::<crate::mcp_admin::AgentState>();
        let session = session_id.unwrap_or_else(|| "default".to_string());
        let hour = chrono::Local::now().hour() as u32;
        let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
        // Short lock for the gate + kit build; the turn locks per phase.
        let mut kit = {
            let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
            crate::onboarding::ensure_not_minor(&conn)?;
            crate::mcp_admin::build_agent_kit(&conn, agent.inner(), &user_text)
        };
        chat_turn_with_kit(
            store.inner(),
            backend.as_mut(),
            &card,
            &session,
            &user_text,
            hour,
            raining.unwrap_or(false),
            calendar.inner().source().access(),
            &mut kit,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Streaming chat command: **fire-and-forget**（2026-11-23，P1）。
///
/// 命令立即返回受理结果，整轮结果通过同一 Channel 的
/// `turn_done`（完整 `DialogueOutcome`）/ `turn_error`（结构化对象）事件回传。
/// 为什么不按旧做法「命令等整轮再 resolve」：一轮真实对话是分钟级
/// （计划轮 1 次 + ≤6 次云端请求，单次上限 120s，+ 工具执行 + 有界重试），
/// 前端任何固定 IPC 兜底都会在它还在正常跑的时候先判失败——2026-11-23 实录
/// 一条查天气的消息后端跑了 28s+，前端 15s 就撤气泡、后端继续落库。
/// 现在前端只按「事件静默」判死（见 useChat 的 IDLE_TIMEOUT_MS）。
///
/// 管线仍在 `spawn_blocking` 里跑（MAIN-THREAD RULE 见上），事件推送本身
/// 是线程安全的 channel send。
#[tauri::command]
pub async fn dialogue_chat_stream(
    app: tauri::AppHandle,
    user_text: String,
    session_id: Option<String>,
    raining: Option<bool>,
    on_delta: tauri::ipc::Channel<serde_json::Value>,
) -> Result<(), String> {
    let done_chan = on_delta.clone();
    tauri::async_runtime::spawn(async move {
        let res = tauri::async_runtime::spawn_blocking(move || {
            let store = app.state::<MemoryStore>();
            let card: Persona = app
                .state::<PersonaState>()
                .lock()
                .map_err(|e| e.to_string())?
                .clone();
            let calendar = app.state::<crate::calendar::CalendarState>();
            let agent = app.state::<crate::mcp_admin::AgentState>();
            let session = session_id.unwrap_or_else(|| "default".to_string());
            let hour = chrono::Local::now().hour() as u32;
            let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
            // Short lock for the gate + kit build; the turn locks per phase.
            let mut kit = {
                let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
                crate::onboarding::ensure_not_minor(&conn)?;
                crate::mcp_admin::build_agent_kit(&conn, agent.inner(), &user_text)
            };
            let mut emit = |delta: &str| {
                let _ = on_delta.send(serde_json::json!({ "delta": delta }));
            };
            // 任务卡进度（v12）：同一 Channel 上的结构化消息——前端按
            // msg.task_step 分流渲染，不进正文气泡。
            let mut emit_task = |v: serde_json::Value| {
                let _ = on_delta.send(serde_json::json!({ "task_step": v }));
            };
            chat_turn_streaming_with_kit(
                store.inner(),
                backend.as_mut(),
                &card,
                &session,
                &user_text,
                hour,
                raining.unwrap_or(false),
                calendar.inner().source().access(),
                &mut kit,
                &mut emit,
                &mut emit_task,
            )
            .map_err(|e| e.to_string())
        })
        .await;
        // turn_error 结构化（plan-retry §7.1）：kind/message/retryable，
        // 前端不再整句 includes 猜传输/限流。管道层错误归 internal。
        let payload = match res {
            Ok(Ok(outcome)) => serde_json::json!({ "turn_done": outcome }),
            Ok(Err(e)) => serde_json::json!({
                "turn_error": {
                    "kind": "internal",
                    "message": e,
                    "retryable": true,
                }
            }),
            Err(e) => serde_json::json!({
                "turn_error": {
                    "kind": "internal",
                    "message": e.to_string(),
                    "retryable": true,
                }
            }),
        };
        let _ = done_chan.send(payload);
    });
    Ok(())
}

/// Manual proactive trigger (frontend button / testing). Bypasses the daily
/// quota on purpose: an explicit user request is not proactivity.
///
/// Also a full model round → spawn_blocking (MAIN-THREAD RULE above).
#[tauri::command]
pub async fn dialogue_proactive_now(
    app: tauri::AppHandle,
) -> Result<Option<ProactiveReply>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<MemoryStore>();
        let card: Persona = app
            .state::<PersonaState>()
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        let hour = chrono::Local::now().hour() as u32;
        let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
        {
            let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
            crate::onboarding::ensure_not_minor(&conn)?;
        }
        // proactive_text locks per phase itself — no guard held here.
        Ok(proactive_text(store.inner(), backend.as_mut(), &card, hour, false)
            .map_err(|e| e.to_string())?
            .map(|(text, turn_id)| ProactiveReply { text, turn_id }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Startup replay for the chat page: the recent dialogue stream as flat
/// bubbles (oldest first). Read-only — no minor gate needed (nothing is
/// sent to a model), no session pairing. `limit` defaults to 30;
/// `before_id` is the scroll-up pagination cursor (id < before_id).
#[tauri::command]
pub fn dialogue_history(
    store: tauri::State<'_, MemoryStore>,
    session_id: Option<String>,
    limit: Option<i64>,
    before_id: Option<i64>,
) -> Result<Vec<db::DialogueBubble>, String> {
    let session = session_id.unwrap_or_else(|| "default".to_string());
    store
        .inner()
        .with_read(|conn| {
            db::dialogue_bubbles(
                conn,
                &session,
                limit.unwrap_or(30).clamp(1, 200) as usize,
                before_id,
            )
        })
        .map_err(|e| e.to_string())
}

/// Full generation trace of one turn (spec-s8). Read-only; `None` = the turn
/// predates the trace tables (frontend shows the honest empty state).
#[tauri::command]
pub fn turn_trace_get(
    store: tauri::State<'_, MemoryStore>,
    turn_id: i64,
) -> Result<Option<db::TurnTraceRow>, String> {
    store
        .inner()
        .with_read(|conn| db::turn_trace_get(conn, turn_id))
        .map_err(|e| e.to_string())
}

/// Reverse lookup by dialogue event id (either side of the pair) — the trace
/// page's entry point for bubbles that only carry an event id (history
/// replay). Read-only.
#[tauri::command]
pub fn turn_trace_find_by_event(
    store: tauri::State<'_, MemoryStore>,
    event_id: i64,
) -> Result<Option<i64>, String> {
    store
        .inner()
        .with_read(|conn| db::turn_trace_find_by_event(conn, event_id))
        .map_err(|e| e.to_string())
}

