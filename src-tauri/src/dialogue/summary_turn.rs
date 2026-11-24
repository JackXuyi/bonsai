//! Summary-turn orchestration shell (engine lives in `crate::summary`).

use rusqlite::Connection;

use crate::generation;
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::persona::eval::DialogueBackend;

use super::persist::{filter_json, persist_model_turn};
use super::trace::TraceRecorder;
use super::types::DialogueOutcome;

/// 总结模式温度：比对话低一档——总结要贴着素材，不需要发挥。
pub(crate) const SUMMARY_TEMPERATURE: f64 = 0.4;

/// 一轮总结（plan-summary-skill）。素材在锁内聚合，模型调用在锁外（可能
/// 十几秒），结果照常落库并进 trace。素材为空时不叫模型：没有事实可依据
/// 的总结只能是编造。
#[allow(clippy::too_many_arguments)]
pub(crate) fn summary_turn(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    session_id: &str,
    user_text: &str,
    kind: crate::summary::SummaryKind,
    packs: &[crate::skill::SkillPack],
    base_system: &str,
    rec: &mut TraceRecorder,
    mut on_delta: Option<&mut dyn FnMut(&str)>,
) -> rusqlite::Result<DialogueOutcome> {
    let now = crate::memory::clock::now_local();
    rec.simple("summary_intent", "总结意图路由", kind.action(), None);
    let (digest, system, prompt) = {
        let conn = store.write_guard()?;
        // 「20 日的日报」这类带具体日期的请求按那个日期开窗，不是永远今天。
        let w = crate::summary::resolve_window(user_text, kind, now);
        let t_recall = std::time::Instant::now();
        let d = crate::summary::collect_in(&conn, kind, w.clone());
        // 流程第 2 步（memory_search 整窗召回）记成正式 tool_call，返回
        // 与真工具同形状的详细 JSON（2026-11-22 用户裁决：否则看不出总结
        // 基于哪些消息）。args 是解析出的窗口；lines 是喂给模型的素材
        // 原文（80 字截断）；tool_calls 审计行照旧只存计数。
        rec.tool_call(
            "memory_search",
            &format!(
                "{{\"kind\":\"{}\",\"start\":\"{}\",\"end_exclusive\":\"{}\"}}",
                kind.action(),
                w.start_day(),
                w.end
                    .succ_opt()
                    .map(|e| e.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            ),
            &d.recall_full,
            "ok",
            None,
            t_recall.elapsed(),
        );
        let contract = crate::summary::contract(kind, packs);
        let (s, u) = crate::summary::prompts(kind, base_system, &contract, &d);
        (d, s, u)
    };

    let mut model_used = false;
    let text = if digest.is_empty() {
        crate::summary::empty_text(kind)
    } else {
        let t = std::time::Instant::now();
        let raw = match on_delta.as_deref_mut() {
            Some(cb) => backend.reply_streaming(&system, &[], &prompt, SUMMARY_TEMPERATURE, cb),
            None => backend.reply(&system, &[], &prompt, SUMMARY_TEMPERATURE),
        };
        rec.llm_round("总结成文", &system, &raw, "answered", t.elapsed());
        model_used = !raw.trim().is_empty();
        if model_used {
            raw.trim().to_string()
        } else {
            crate::summary::fallback(&digest)
        }
    };
    // 流式路径下模型可能什么都没吐（挂了）——回落文本要补发一次，否则
    // 气泡一直是空的。
    if !model_used {
        if let Some(cb) = on_delta.as_deref_mut() {
            cb(&text);
        }
    }

    let filter = generation::filter(&text);
    rec.simple("filter", "反谄媚过滤", "pass", None);
    let tool = format!("builtin:{}", kind.action());
    let conn = store.write_guard()?;
    // 总结也记一次工具使用：调用记录页可见，周报的「用过工具」也有据。
    // detail 只放计数，不放内容。
    let _ = db::log_tool_call(
        &conn,
        "summary",
        kind.action(),
        "ok",
        Some(&format!(
            "turns={} facts={}",
            digest.turns.len(),
            digest.facts.len()
        )),
        None,
    );
    let user_id =
        db::add_event_meta(&conn, "dialogue", user_text, Some("user"), Some(session_id))?;
    let asst_id = db::add_event_meta(&conn, "dialogue", &text, Some("assistant"), Some(session_id))?;
    let turn_id = rec.persist(
        &conn,
        session_id,
        Some(user_id),
        Some(asst_id),
        "model",
        model_used,
        Some(tool.clone()),
        Some(filter_json(&filter)),
        Some(user_text),
        Some(&text),
    )?;
    // 总结不是立场表达：filter 报告照记，但不写 disagreement 事件——那是
    // 关系线「顶嘴」的证据，总结里出现「不同意」字样不该算数。
    Ok(DialogueOutcome {
        reply: text,
        filter,
        model_used,
        crisis: false,
        tool: Some(tool),
        task: None,
        turn_id: Some(turn_id),
        failure: None,
    })
}

