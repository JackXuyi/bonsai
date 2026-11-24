//! Chat turn entry points and pipeline orchestrator (`chat_turn_inner`).

use rusqlite::Connection;

use crate::face::{self, FaceState};
use crate::generation;
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::persona::{eval::DialogueBackend, Persona};
use crate::relationship;

use super::cloud_loop::{cloud_tool_loop, skill_section, tool_label};
use super::local_legs::{local_perception_answer, local_recall_answer};
use super::persist::{clip, filter_json, lock_conn, persist_model_turn};
use super::prompt::{system_prompt, turn_budget, turns_to_history, env_prefix};
use super::summary_turn::summary_turn;
use super::trace::TraceRecorder;
use super::types::{
    resolve_model_reply, DialogueOutcome, TaskCard, CHAT_TEMPERATURE, PROACTIVE_SESSION,
};

/// One user-initiated chat turn: render → backend → filter → record.
/// Always produces a reply (fallback line when the model is down).
///
/// Takes `&MemoryStore` and locks it per DB phase (lock-narrowing, 2026-11-20):
/// the model rounds below run with ZERO locks held, so a 45s cloud inference
/// never blocks the proactive thread, tray updates or consolidate jobs.
pub fn chat_turn(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    access: crate::calendar::CalendarAccess,
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(store, backend, card, session_id, user_text, hour, raining, access, None, None, None)
}

/// Same turn, with the agent pipeline (spec-s3): three-stage progressive
/// tool/skill selection runs only when the kit carries capabilities.
pub fn chat_turn_with_kit(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    access: crate::calendar::CalendarAccess,
    kit: &mut crate::mcp_admin::AgentKit,
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(store, backend, card, session_id, user_text, hour, raining, access, Some(kit), None, None)
}

/// Same turn with incremental delivery: every model chunk goes to `on_delta`
/// while it is still being generated (the frontend renders it live).
pub fn chat_turn_streaming(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    access: crate::calendar::CalendarAccess,
    on_delta: &mut dyn FnMut(&str),
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(store, backend, card, session_id, user_text, hour, raining, access, None, Some(on_delta), None)
}

/// Streaming + agent pipeline: deltas fire on the final round only
/// (spec-s3 deviation note — the selection rounds may be JSON).
pub fn chat_turn_streaming_with_kit(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    access: crate::calendar::CalendarAccess,
    kit: &mut crate::mcp_admin::AgentKit,
    on_delta: &mut dyn FnMut(&str),
    on_task: &mut dyn FnMut(serde_json::Value),
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(
        store, backend, card, session_id, user_text, hour, raining, access,
        Some(kit), Some(on_delta), Some(on_task),
    )
}

/// 直答的唯一调用点（plan-retry §5.1）：调 `reply*`，把 `take_failure` 的
/// 结果写进 trace **并交回给轮层**。成功记 `answered`；流式中途断（正文
/// 非空 + 失败）记 `interrupted`；全失败记 `error`，output 从空串变成
/// `kind=… attempts=… detail…`——401 与 502 从此可区分。
pub(crate) fn call_backend_with_trace(
    backend: &mut dyn DialogueBackend,
    system: &str,
    history: &[crate::persona::eval::Turn],
    user_text: &str,
    temperature: f64,
    mut on_delta: Option<&mut dyn FnMut(&str)>,
    rec: &mut TraceRecorder,
) -> (String, Option<crate::failure::Failure>) {
    // 防御：规则路由 / 总结路径留下的失败不许串到这一轮（§11 风险矩阵）。
    let _ = backend.take_failure();
    let t = std::time::Instant::now();
    let raw = match on_delta.as_deref_mut() {
        Some(cb) => backend.reply_streaming(system, history, user_text, temperature, cb),
        None => backend.reply(system, history, user_text, temperature),
    };
    let f = backend.take_failure();
    let outcome = match &f {
        Some(_) if !raw.trim().is_empty() => "interrupted",
        Some(_) => "error",
        None => "answered",
    };
    let output = match &f {
        Some(x) => format!("kind={} attempts={} {}", x.kind.as_str(), x.attempts, x.detail),
        None => raw.clone(),
    };
    rec.llm_round("直接对话", system, &output, outcome, t.elapsed());
    (raw, f)
}

pub(crate) fn chat_turn_inner(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    access: crate::calendar::CalendarAccess,
    mut kit: Option<&mut crate::mcp_admin::AgentKit>,
    mut on_delta: Option<&mut dyn FnMut(&str)>,
    mut on_task: Option<&mut dyn FnMut(serde_json::Value)>,
) -> rusqlite::Result<DialogueOutcome> {
    // Trace collector (spec-s8): lives for the whole turn; every path below
    // records its steps and persists the draft together with the dialogue
    // events (same lock scope, one transaction). 当前输入进每个 step
    // （2026-11-21：user message 不在 system prompt 里，trace 必须自带）。
    let mut rec = TraceRecorder::with_user(user_text);

    // 链路第 0 步（用户裁决 2026-11-22）：追踪必须从输入的对话开始。全文
    // 进 output（trace 是本地调试面），展开即见；每步的 user_text 字段
    // 照旧冗余携带。
    rec.simple("user_input", "用户输入", "", Some(user_text));

    // Crisis intercept (plan-onboarding.md §3.3): rule-level, BEFORE any
    // model call — a dead model must not change this behavior. Both sides
    // are still recorded (compliance audit), no streaming deltas are sent.
    let crisis = crate::onboarding::crisis_hit(user_text);
    rec.simple(
        "crisis_check",
        "危机拦截检查",
        if crisis { "hit" } else { "miss" },
        if crisis { Some(user_text) } else { None },
    );
    if crisis {
        let reply = crate::onboarding::CRISIS_REPLY.to_string();
        let conn = lock_conn(store)?;
        let user_id =
            db::add_event_meta(&conn, "dialogue", user_text, Some("user"), Some(session_id))?;
        let asst_id =
            db::add_event_meta(&conn, "dialogue", &reply, Some("assistant"), Some(session_id))?;
        let turn_id = rec.persist(
            &conn,
            session_id,
            Some(user_id),
            Some(asst_id),
            "crisis",
            false,
            None,
            None,
            Some(user_text),
            Some(&reply),
        )?;
        return Ok(DialogueOutcome {
            reply,
            filter: generation::FilterReport::default(),
            model_used: false,
            crisis: true,
            tool: None,
            task: None,
            turn_id: Some(turn_id),
            failure: None,
        });
    }

    // Tool intents (plan-calendar.md §3.5) sit between the crisis check and
    // the model: a schedule question is answered from the calendar cache by
    // deterministic rules, so it works with the local model down and never
    // invents a meeting. A miss falls straight through to normal chat.
    // One lock scope: cache read/write + both event rows (ids captured here
    // for the trace backfill — spec-s8 §3.2).
    let tool_answer: Option<(crate::tools::ToolReply, i64, i64)> = {
        let conn = lock_conn(store)?;
        match crate::tools::maybe_answer(&conn, access, user_text, chrono::Local::now()) {
            None => {
                rec.simple("tool_intent", "日历规则路由", "miss", None);
                None
            }
            Some(t) => {
                rec.simple("tool_intent", "日历规则路由", "hit", Some(&t.reply));
                let user_id = db::add_event_meta(
                    &conn,
                    "dialogue",
                    user_text,
                    Some("user"),
                    Some(session_id),
                )?;
                let asst_id = db::add_event_meta(
                    &conn,
                    "dialogue",
                    &t.reply,
                    Some("assistant"),
                    Some(session_id),
                )?;
                Some((t, user_id, asst_id))
            }
        }
    };
    if let Some((tool, user_id, asst_id)) = tool_answer {
        let conn = lock_conn(store)?;
        let turn_id = rec.persist(
            &conn,
            session_id,
            Some(user_id),
            Some(asst_id),
            "tool",
            false,
            Some(format!("calendar:{}", tool.action)),
            None,
            Some(user_text),
            Some(&tool.reply),
        )?;
        return Ok(DialogueOutcome {
            reply: tool.reply,
            filter: generation::FilterReport::default(),
            model_used: false,
            crisis: false,
            tool: Some(tool.action.to_string()),
            task: None,
            turn_id: Some(turn_id),
            failure: None,
        });
    }

    // ---- phase: context read (one short lock) → then inference runs FREE
    // 装配统一走 `context::build`（plan-context §6.3，T2）：预算充足时与旧
    // 行为逐条等价（system = env + body，历史原样），装不下时从最旧开始丢，
    // 最近 2 轮必留。这里仍是**唯一**装配点——云端循环 / 本地规则路由 /
    // 直答 / 总结全部消费这一份 system + history。
    let will_use_tools = kit.as_deref().map(|k| k.has_capabilities()).unwrap_or(false);
    let (system, history) = {
        let conn = lock_conn(store)?;
        let env = env_prefix(&conn);
        let body = system_prompt(&conn, card, hour, raining)?;
        let turns = db::dialogue_turns_meta(&conn, session_id, crate::context::HISTORY_MAX_TURNS)?;
        let (budget, cloud) = turn_budget(&conn);
        // 云端工具循环会往消息序列里插 `tool` 消息 → 必须预留；本地零工具
        // 调用，预留只会白占 2B 的 8192 窗口。
        let reserve = cloud && will_use_tools;
        let (digest, upto) = db::session_digest(&conn, session_id).unwrap_or((None, None));

        // 装一遍先看会丢几轮（纯计算，微秒级）。
        let mut built = crate::context::build(
            crate::context::BuildInput {
                env: &env,
                body: &body,
                digest: digest.as_deref(),
                history: turns_to_history(&turns),
                user_text,
                reserve_tool_room: reserve,
            },
            budget,
        );

        // L1（§6.4）：真丢了轮才写摘要，且只合并水位线之后的轮——重复调用
        // 不会重复加行（A6）。摘要比丢掉的轮还旧的那部分本来就在 digest 里。
        if built.dropped > 0 {
            let fresh: Vec<crate::context::DigestTurn> = turns
                .iter()
                .take(built.dropped)
                .filter(|t| upto.map_or(true, |u| t.upto_id > u))
                .map(|t| crate::context::DigestTurn {
                    day: &t.day,
                    user: &t.user,
                    assistant: &t.assistant,
                    upto_id: t.upto_id,
                })
                .collect();
            if !fresh.is_empty() {
                let merged = crate::context::digest_merge(digest.as_deref(), &fresh);
                let new_upto = fresh.iter().map(|t| t.upto_id).max().unwrap_or(0);
                // 写库失败不挡这一轮：摘要是增益，丢了下一轮还会再试。
                // 会话行不存在（proactive）时这里静默 0 行，正是想要的。
                let _ = db::session_set_digest(&conn, session_id, &merged, new_upto);
                // 用刚合并的摘要重建一次：否则这一轮的模型既看不到那几轮历史、
                // 也看不到摘要，等于凭空失忆一轮。
                built = crate::context::build(
                    crate::context::BuildInput {
                        env: &env,
                        body: &body,
                        digest: Some(&merged),
                        history: turns_to_history(&turns),
                        user_text,
                        reserve_tool_room: reserve,
                    },
                    budget,
                );
            }
        }

        // 观测（A10 的数据来源）：估算值 / 丢弃轮数 / 两个预算数字。
        rec.simple(
            "context_budget",
            "上下文预算",
            if built.dropped > 0 { "trimmed" } else { "ok" },
            Some(&format!(
                "est={} usable={} window={} dropped={} digest={}",
                built.est_tokens,
                budget.usable,
                budget.window,
                built.dropped,
                built.system.contains(crate::context::DIGEST_HEADER)
            )),
        );
        (built.system, built.turns)
    };

    // 总结意图（plan-summary-skill）：规则命中就接管这一轮——素材由 Rust
    // 聚合，模型只负责成文；模型挂了还有确定性回落，所以本地/云端同一条
    // 路，不需要工具调用能力。放在日历路由之后：tools.rs 的排除词已经把
    // 「总结」类句子让出来了。
    if let Some(kind) = crate::summary::intent(user_text) {
        let packs: &[crate::skill::SkillPack] = kit
            .as_deref()
            .map(|k| k.engine_skills.as_slice())
            .unwrap_or(&[]);
        return summary_turn(
            store, backend, session_id, user_text, kind, packs, &system, &mut rec, on_delta,
        );
    }

    // 工具调用（plan-task-card v0.2 §2 分流）：kit 有能力时分两条路——
    // 云端传输 → 原生 function calling 循环；本地传输 → 零模型工具调用
    // （recall 代码路由 + 直答）。Anything it returns is final for the
    // turn — record both sides, filter, out.
    if let Some(kit) = kit.take().filter(|k| k.has_capabilities()) {
        let mut reborrow = on_delta.as_mut().map(|f| &mut **f as &mut dyn FnMut(&str));
        let reborrow = &mut reborrow;
        let (raw, tool_label, task_card, streamed, loop_fail) = match backend.tool_client() {
            Some(client) => {
                kit.trace("mode:cloud".into());
                cloud_tool_loop(
                    store, client, kit, &system, &history, user_text, session_id, &mut rec,
                    on_task.take(),
                )?
            }
            None => {
                kit.trace("mode:local".into());
                // 本地模式（2026-11-22 裁决）：模型不选工具。代码层强制路由
                // 两枚内置工具（None = 回落直答）：recall 信号 → memory_search；
                // perceive 信号 → perception_search（plan-perception-tool §5.2）。
                match local_recall_answer(
                    store, backend, kit, &system, &history, user_text, &mut rec,
                    reborrow.as_mut().map(|f| &mut **f as &mut dyn FnMut(&str)),
                )? {
                    Some(triple) => (triple.0, triple.1, None, triple.2, None),
                    None => {
                        match local_perception_answer(
                            store, backend, kit, &system, &history, user_text, &mut rec,
                            reborrow.as_mut().map(|f| &mut **f as &mut dyn FnMut(&str)),
                        )? {
                            Some(triple) => (triple.0, triple.1, None, triple.2, None),
                            None => {
                                // 直答：Failure 进 trace 并交回轮层（T2/T7）。
                                let mut reborrow = on_delta.as_deref_mut();
                                let (raw, f) = call_backend_with_trace(
                                    backend, &system, &history, user_text, CHAT_TEMPERATURE,
                                    reborrow.as_mut().map(|f| &mut **f as &mut dyn FnMut(&str)),
                                    &mut rec,
                                );
                                (raw, None, None, true, f)
                            }
                        }
                    }
                }
            }
        };
        let (reply, model_used, failure) = resolve_model_reply(raw, loop_fail);
        // Rounds 1/2 and template replies were not streamed — emit once.
        // 失败 hint / 本地拼装也要推一次，否则气泡只剩空壳。
        if !streamed {
            if let Some(cb) = on_delta.as_mut() {
                if !reply.trim().is_empty() {
                    cb(&reply);
                }
            }
        }
        let filter = generation::filter(&reply);
        let turn_id = persist_model_turn(
            store, &mut rec, session_id, user_text, &reply, model_used, tool_label.clone(),
            Some(&filter),
        )?;
        if model_used {
            let conn = lock_conn(store)?;
            if filter.explicit_disagreement {
                db::add_event(&conn, "disagreement", &clip(&reply, 80))?;
            }
            if filter.hedged_disagreement {
                db::add_event(&conn, "disagreement_hedged", &clip(&reply, 80))?;
                let _ = relationship::maybe_first_refusal(&conn, chrono::Local::now());
            }
        }
        return Ok(DialogueOutcome {
            reply,
            filter,
            model_used,
            crisis: false,
            tool: tool_label,
            task: task_card,
            turn_id: Some(turn_id),
            failure,
        });
    }

    let mut reborrow = on_delta.as_deref_mut();
    let (raw, f) = call_backend_with_trace(
        backend, &system, &history, user_text, CHAT_TEMPERATURE,
        reborrow.as_mut().map(|f| &mut **f as &mut dyn FnMut(&str)),
        &mut rec,
    );
    let (reply, model_used, failure) = resolve_model_reply(raw, f);
    let filter = generation::filter(&reply);
    let turn_id = persist_model_turn(
        store, &mut rec, session_id, user_text, &reply, model_used, None, Some(&filter),
    )?;

    // Anti-sycophancy evidence (plan-relationship.md §3.6): the filter used to
    // be report-only, so "how often does it actually push back" was
    // unknowable. Record the two shapes as events — explicit push-back is the
    // relationship line's only qualitative evidence.
    if model_used {
        let conn = lock_conn(store)?;
        if filter.explicit_disagreement {
            db::add_event(&conn, "disagreement", &clip(&reply, 80))?;
        }
        if filter.hedged_disagreement {
            db::add_event(&conn, "disagreement_hedged", &clip(&reply, 80))?;
            // The wishy-washy moment may be the one worth telling a story about
            // (R2 + 7 days, fired at most once, ever).
            let _ = relationship::maybe_first_refusal(&conn, chrono::Local::now());
        }
    }
    Ok(DialogueOutcome {
        reply,
        filter,
        model_used,
        crisis: false,
        tool: None,
        task: None,
        turn_id: Some(turn_id),
        failure,
    })
}

