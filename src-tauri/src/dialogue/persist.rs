//! Short store locks and model-turn persistence.

use rusqlite::Connection;

use crate::generation;
use crate::memory::db;
use crate::memory::MemoryStore;

use super::trace::TraceRecorder;

/// FilterReport → the JSON stored in `turn_trace.filter_report`.
pub(crate) fn filter_json(filter: &generation::FilterReport) -> String {
    serde_json::json!({
        "unconditional_affirmation": filter.unconditional_affirmation,
        "hedged": filter.hedged_disagreement,
        "explicit": filter.explicit_disagreement,
    })
    .to_string()
}

/// §5.3 失败轮落库（D1）：完全失败（无正文）**不落 assistant 事件**——兜底句
/// 只回 UI 不进历史，`dialogue_history` / digest / 气泡三处零改动自动免疫；
/// 部分成功正文照落。trace 同规则：空正文 `reply_text` 传 None，失败详情由
/// error 步的 `kind=… attempts=…` 说话。反谄媚过滤只对真正文有意义。
#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_model_turn(
    store: &MemoryStore,
    rec: &mut TraceRecorder,
    session_id: &str,
    user_text: &str,
    reply: &str,
    model_used: bool,
    tool_label: Option<String>,
    filter: Option<&generation::FilterReport>,
) -> rusqlite::Result<i64> {
    if model_used {
        rec.simple("filter", "反谄媚过滤", "pass", None);
    }
    let conn = lock_conn(store)?;
    let user_id =
        db::add_event_meta(&conn, "dialogue", user_text, Some("user"), Some(session_id))?;
    let asst_id = if model_used {
        Some(db::add_event_meta(
            &conn,
            "dialogue",
            reply,
            Some("assistant"),
            Some(session_id),
        )?)
    } else {
        None
    };
    rec.persist(
        &conn,
        session_id,
        Some(user_id),
        asst_id,
        "model",
        model_used,
        tool_label,
        filter.filter(|_| model_used).map(|f| filter_json(f)),
        Some(user_text),
        if model_used { Some(reply) } else { None },
    )
}

/// First `n` characters — evidence snippets, not full transcripts.
pub(crate) fn clip(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Short-lived store lock for ONE db operation (2026-11-20 lock-narrowing).
/// The chat pipeline acquires/releases per phase: model inference, MCP
/// subprocess waits and skill-body reads NEVER hold the store mutex.
/// The guard derefs to `Connection`, so every db-layer helper keeps its
/// `&Connection` signature unchanged.
pub(crate) fn lock_conn(store: &MemoryStore) -> rusqlite::Result<std::sync::MutexGuard<'_, Connection>> {
    store.0.lock().map_err(|_| {
        // plan-retry §5.6：中毒不做 unsafe 恢复，但出口必须可辨识——
        // 前缀 internal_poisoned 让前端/日志能把它与普通 sqlite 错误区分开
        //（对应 FailureKind::Internal 的「内部出错，重启一下试试」）。
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("internal_poisoned: memory store mutex poisoned — 内部状态已损坏，请重启应用".into()),
        )
    })
}

