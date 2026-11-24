//! Local-mode rule legs: memory/perception recall without model tool calling.

use crate::memory::MemoryStore;
use crate::persona::eval::DialogueBackend;

use super::cloud_loop::{kit_task_id, tool_label};
use super::trace::TraceRecorder;
use super::types::CHAT_TEMPERATURE;

/// 本地模式的 recall 路由（v0.2 §5，D1 裁决）：本地模型不做工具调用，
/// 但「总结一下我 N 号说了啥」不能靠编——recall 信号命中时由**代码层**
/// 强制执行 memory_search，摘要交给人格轮措辞（沿 pass-3 解读机制）。
/// 执行失败回落直答（审计行已落，不静默）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn local_recall_answer(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    kit: &mut crate::mcp_admin::AgentKit,
    system: &str,
    history: &[crate::persona::eval::Turn],
    user_text: &str,
    rec: &mut TraceRecorder,
    on_delta: Option<&mut dyn FnMut(&str)>,
) -> rusqlite::Result<Option<(String, Option<String>, bool)>> {
    if !kit.catalog_all().iter().any(|d| d.name == "memory_search") {
        return Ok(None);
    }
    kit.trace("local:forced-memory-search".into());
    // 按日期筛选（2026-11-21 裁决沿用）：日期窗在服务端解析并显式进参数。
    let args = memory_search_args(user_text);
    let args_text = args.to_string();
    let t_tool = std::time::Instant::now();
    match crate::mcp_admin::execute_audited(store, kit, "memory_search", args) {
        Err(e) => {
            rec.tool_call("memory_search", &args_text, "", "error", Some(&e), t_tool.elapsed());
            Ok(None) // 回落直答
        }
        Ok(out) => Ok(Some(summarize_tool_result(
            backend,
            kit,
            history,
            user_text,
            system,
            "memory_search",
            &args_text,
            Some("builtin:memory_search".to_string()),
            &out,
            t_tool.elapsed(),
            rec,
            on_delta,
        )?)),
    }
}

/// 本地模式的感知路由（plan-perception-tool §5.2）：与 local_recall_answer
/// 同构——本地模型不做工具调用，`might_perceive` 信号命中时由**代码层**
/// 强制执行 perception_search，摘要交给人格轮措辞（同一条 pass-3）。
/// 执行失败回落直答（审计行已落，不静默）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn local_perception_answer(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    kit: &mut crate::mcp_admin::AgentKit,
    system: &str,
    history: &[crate::persona::eval::Turn],
    user_text: &str,
    rec: &mut TraceRecorder,
    on_delta: Option<&mut dyn FnMut(&str)>,
) -> rusqlite::Result<Option<(String, Option<String>, bool)>> {
    if !kit
        .catalog_all()
        .iter()
        .any(|d| d.name == "perception_search")
    {
        return Ok(None);
    }
    kit.trace("local:forced-perception-search".into());
    // 按日期筛选（沿 memory_search 裁决）：日期窗在服务端解析并显式进参数。
    let args = memory_search_args(user_text);
    let args_text = args.to_string();
    let t_tool = std::time::Instant::now();
    match crate::mcp_admin::execute_audited(store, kit, "perception_search", args) {
        Err(e) => {
            rec.tool_call("perception_search", &args_text, "", "error", Some(&e), t_tool.elapsed());
            Ok(None) // 回落直答
        }
        Ok(out) => Ok(Some(summarize_tool_result(
            backend,
            kit,
            history,
            user_text,
            system,
            "perception_search",
            &args_text,
            Some("builtin:perception_search".to_string()),
            &out,
            t_tool.elapsed(),
            rec,
            on_delta,
        )?)),
    }
}

/// Build the pass-3 (result-summary) system prompt. When the tool's summary
/// is itself JSON (prune tools return JSON payloads; ToolOutput.summary is
/// the clipped raw text), the prompt must explicitly demand INTERPRETATION —
/// "请基于摘要简短回答" over a JSON blob makes the model echo the blob back
/// as its "answer" (2026-11-20 mem_leaks regression: the user got the raw
/// JSON in the fallback template because both pass-3 and the retry echoed).
pub(crate) fn summary_round_prompt(sys1: &str, name: &str, summary: &str) -> String {
    let t = summary.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        format!(
            "{sys1}\n\n【工具 {name} 的结果（JSON 数据）】\n{summary}\n\n上面是 JSON 数据，不是答案。请解读它的字段含义，用中文口语向用户给出结论：关键数字、你的判断、下一步建议。不要输出 JSON，不要逐字复述结构。"
        )
    } else {
        format!(
            "{sys1}\n\n【工具 {name} 的结果摘要】\n{summary}\n\n请基于摘要简短回答用户；不要复述摘要全文，不要编造摘要之外的信息。"
        )
    }
}

/// pass 3: result-summary round (streaming lives here). Shared by the normal
/// `ToolDecision::Call` path and the forced memory-search fallback (2026-11-21:
/// recall 直答兜底走同一条解读路径，避免两份 pass-3 拷贝漂移).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(crate) fn summarize_tool_result(
    backend: &mut dyn DialogueBackend,
    kit: &mut crate::mcp_admin::AgentKit,
    history: &[crate::persona::eval::Turn],
    user_text: &str,
    base_sys: &str,
    name: &str,
    args_text: &str,
    label: Option<String>,
    out: &crate::agent_tools::ToolOutput,
    tool_dur: std::time::Duration,
    rec: &mut TraceRecorder,
    mut on_delta: Option<&mut dyn FnMut(&str)>,
) -> rusqlite::Result<(String, Option<String>, bool)> {
    let sys2 = summary_round_prompt(base_sys, name, &out.summary);
    kit.trace("level3:answered".into());
    // trace 存 full 而非 summary（2026-11-21 用户裁决：工具 trace 数据一律
    // JSON，/trace 页结构化渲染）；spec D1 本就是 FULL TEXT 留痕语义。
    rec.tool_call(name, args_text, &out.full, "ok", None, tool_dur);
    // Third JSON-leak guard (2026-11-20 regression trio): the
    // model can echo its own pass-2 decision JSON instead of
    // prose. Streaming deltas are IRRETRACTABLE, so the sink
    // gates on the first non-whitespace char: `{` mutes the
    // whole stream (nothing reaches the UI) and triggers one
    // non-streamed prose retry.
    let t3 = std::time::Instant::now();
    let (raw3, json_out) = match on_delta.as_deref_mut() {
        Some(cb) => {
            let verdict = std::cell::RefCell::new(None::<bool>); // None=undecided, Some(true)=json
            let mut sink = |d: &str| {
                let is_json = {
                    let mut g = verdict.borrow_mut();
                    match *g {
                        Some(true) => return,
                        Some(false) => false,
                        None => match d.chars().find(|c| !c.is_whitespace()) {
                            None => return,
                            Some(c) => {
                                let json = c == '{';
                                *g = Some(json);
                                json
                            }
                        },
                    }
                };
                if is_json {
                    return;
                }
                cb(d);
            };
            let raw3 = backend.reply_streaming(&sys2, history, user_text, CHAT_TEMPERATURE, &mut sink);
            let json_out = matches!(*verdict.borrow(), Some(true)) || raw3.trim_start().starts_with('{');
            (raw3, json_out)
        }
        None => {
            let raw3 = backend.reply(&sys2, history, user_text, CHAT_TEMPERATURE);
            let json_out = raw3.trim_start().starts_with('{');
            (raw3, json_out)
        }
    };
    rec.llm_round(
        &format!("第 3 轮 · 解读 {name} 结果"),
        &sys2,
        &raw3,
        "answered",
        t3.elapsed(),
    );
    if raw3.trim().is_empty() {
        Ok((
            format!("（{name} 跑完了，但盆栽没组织好语言——结果：{}）", out.summary),
            label,
            true,
        ))
    } else if json_out {
        kit.trace("level3:json-not-prose".into());
        let sys3 = format!(
            "{sys2}\n\n（你刚才的回复是 JSON，这不对。{} 请直接用自然语言、面向用户回答；不要输出任何 JSON 或代码块。）",
            if out.summary.trim_start().starts_with('{') || out.summary.trim_start().starts_with('[') {
                "解读上面的 JSON 数据，给出结论。"
            } else {
                ""
            }
        );
        let retry = backend.reply(&sys3, history, user_text, CHAT_TEMPERATURE);
        rec.simple(
            "json_retry",
            "JSON 泄漏 · 重问直答",
            "ok",
            Some(&retry),
        );
        if !retry.trim().is_empty() && !retry.trim_start().starts_with('{') {
            Ok((retry, label, false))
        } else {
            Ok((
                format!("（{name} 的结果：{}）", out.summary),
                label,
                false,
            ))
        }
    } else {
        Ok((raw3, label, true))
    }
}

/// memory_search 的调用参数（2026-11-21 用户裁决：按日期筛选要显式）：
/// 服务端解析日期窗 → date_from/date_to（含语义）+ query 原话；解析不出
/// 日期短语才只传 query（execute 侧自带最近一周兜底）。强制检索兜底与
/// pass-2 candidates 复读转决策共用。
pub(crate) fn memory_search_args(user_text: &str) -> serde_json::Value {
    let today = crate::memory::clock::now_local().date_naive();
    match crate::memory_search::parse_window(user_text, today) {
        Some((s, e, _)) => serde_json::json!({
            "query": user_text,
            "date_from": s.format("%Y-%m-%d").to_string(),
            "date_to": e.pred_opt()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| s.format("%Y-%m-%d").to_string()),
        }),
        None => serde_json::json!({ "query": user_text }),
    }
}

