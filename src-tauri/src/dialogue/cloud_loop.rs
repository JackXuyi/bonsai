//! Cloud tool loop + plan / retry helpers (plan-planning / plan-retry).
//!
//! V1 keeps this as one file (plan-dialogue-split D3).

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::memory::db;
use crate::memory::MemoryStore;

use super::persist::clip;
use super::trace::TraceRecorder;
use super::types::{TaskCard, TaskStep, CHAT_TEMPERATURE};

/// T4：read 档失败重试退避（plan-planning §5.4，500ms）。
pub(crate) const RETRY_BACKOFF: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------- tool calling
//
// 工具调用分流（plan-task-card v0.2，2026-11-22 用户裁决）：
//   云端模式 = DeepSeek 原生 function calling 多轮循环（§cloud_tool_loop）；
//   本地模式 = 零模型工具调用，只保留代码层规则路由——日历（chat_turn_inner
//   前置）+ memory_search recall（§local_recall_answer）。为 2B 手搓的三级
//   渐进 JSON 管线（candidates/决策轮）整体退役：解析器仍留在 agent_tools.rs
//   供单测与未来复用。
//
// 人格（D6 裁决）：云端循环的 system prompt 带完整人格宪法（L1/L2/L3 + 环境
// 段 + skill 段）——2B「人格稀释 JSON」的教训不适用于 DeepSeek；循环自然
// 结束时模型直接以阿盆口吻作答，不再需要 pass-C 润色轮。

/// 单轮对话工具调用总次数上限（v0.2 §4 D2）。超出即强制收尾。
pub(crate) const CLOUD_TOOL_CALL_CAP: usize = 8;
/// 单轮对话 HTTP 请求次数上限（含最终作答轮）。网络往返比本地推理贵。
pub(crate) const CLOUD_REQUEST_CAP: usize = 6;

/// 规划意图判定（plan-planning v0.2 §5.1）：纯规则、零模型、零成本。
///
/// **只在云端分支调用**——本地路径永不规划（本地零工具调用硬约束，A9）。
/// 保守判定：宁可漏规划，不可误规划——误规划会多花一次云端请求且打断
/// 单步任务。三级信号：
///   1. 显式规划 cue（「规划一下/列步骤/分几步」）或成对序列词（先…再）→ 命中；
///   2. ≥2 个不同动作动词 且 kit 有 ≥2 工具 → 命中；
///   3. ≥2 个工具域关键词 且 kit 有 ≥2 工具 → 命中。
pub(crate) fn plan_intent(user_text: &str, kit: &crate::mcp_admin::AgentKit) -> bool {
    // 强 cue：明确要求规划/分步/多件事一起做，单个即命中（不依赖 kit）。
    pub(crate) const STRONG: &[&str] = &[
        "规划一下", "帮我规划", "列个步骤", "列出步骤", "列步骤",
        "分几步", "分步骤", "一步步", "一起做", "按顺序",
    ];
    if STRONG.iter().any(|c| user_text.contains(c)) {
        return true;
    }
    // 序列 cue 必须成对（「先…再」「先…然后」）——单字「先/再」太易误伤
    // （「再说」「先这样」）。
    pub(crate) const SEQ: &[&str] = &["先", "再", "然后", "接着", "随后", "之后"];
    if SEQ.iter().filter(|c| user_text.contains(**c)).count() >= 2 {
        return true;
    }
    // 多动作 / 多域都要求 kit 有 ≥2 工具：单工具场景一步就做完，无需规划。
    if kit.catalog_all().len() < 2 {
        return false;
    }
    // 用「不同动作」去重：'查一下' 与 '查' 只算一个动作。
    // 单字动词避免与域名子串冲突（「记」会命中「记忆」）——改用双字「记到/记录」。
    pub(crate) const ACTIONS: &[&str] = &[
        "查", "找", "读", "写", "搜", "发", "存", "记到", "记录",
        "整理", "总结", "合并", "生成", "汇总", "对比", "列出",
        "下载", "打开", "删除", "导出", "导入", "复制", "移动",
    ];
    let actions: std::collections::HashSet<&str> = ACTIONS
        .iter()
        .copied()
        .filter(|a| user_text.contains(a))
        .collect();
    if actions.len() >= 2 {
        return true;
    }
    pub(crate) const DOMAINS: &[&str] = &[
        "文件", "文档", "网页", "网址", "链接", "日程", "会议", "日历",
        "邮件", "记忆", "笔记", "备忘录", "剪贴板", "目录", "文件夹",
    ];
    let domains: std::collections::HashSet<&str> = DOMAINS
        .iter()
        .copied()
        .filter(|d| user_text.contains(d))
        .collect();
    domains.len() >= 2
}

/// 计划步数上限（plan-planning §7）：低于 CLOUD_TOOL_CALL_CAP=8，留 2 步
/// 给「计划外步骤」与收尾。
pub(crate) const MAX_PLAN_STEPS: usize = 6;

/// `submit_plan` 协议工具 spec——只在计划轮传给模型，**不进 provider catalog**
/// （D5 裁决：它是协议工具，不是能力）。
pub(crate) fn submit_plan_spec() -> crate::llm::ToolSpec {
    crate::llm::ToolSpec {
        name: "submit_plan".into(),
        description: "提交本轮多步执行计划。仅在规划轮调用一次；steps 不超过 6 步。".into(),
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "description": "一句话目标" },
                "steps": {
                    "type": "array",
                    "description": "扁平步骤列表，最多 6 步",
                    "items": {
                        "type": "object",
                        "properties": {
                            "intent": { "type": "string", "description": "这一步要做什么" },
                            "tool_hint": { "type": "string", "description": "预期工具名，仅作提示" }
                        },
                        "required": ["intent"]
                    }
                }
            },
            "required": ["goal", "steps"]
        })),
    }
}

/// 把模型提交的 `submit_plan` 参数解析为 PlanJson。
/// 校验：goal 非空、steps 1..=6、每步 intent 非空；tool_hint 缺省填 "*"
/// （表示「计划外/未指定」）。任一不满足 → None（降级为无计划循环）。
pub(crate) fn parse_submitted_plan(args: &serde_json::Value) -> Option<db::PlanJson> {
    let goal = args.get("goal")?.as_str()?.trim().to_string();
    if goal.is_empty() {
        return None;
    }
    let raw_steps = args.get("steps")?.as_array()?;
    if raw_steps.is_empty() || raw_steps.len() > MAX_PLAN_STEPS {
        return None;
    }
    let mut steps = Vec::with_capacity(raw_steps.len());
    for (seq, s) in raw_steps.iter().enumerate() {
        let intent = s.get("intent")?.as_str()?.trim().to_string();
        if intent.is_empty() {
            return None;
        }
        let tool_hint = s
            .get("tool_hint")
            .and_then(|v| v.as_str())
            .unwrap_or("*")
            .to_string();
        steps.push(db::PlanStep {
            seq: seq as i32,
            intent,
            tool_hint,
            status: "pending".into(),
            attempts: 0,
            tool_call_id: None,
        });
    }
    Some(db::PlanJson { version: 1, goal, steps, replans: 0 })
}

/// D1 确认门（plan-planning §5.2）：本版恒放行。
///
/// - `auto`（默认）：计划提交后直接执行，零等待；
/// - `manual`（扩展位）：本版**同样放行**，只把模式写进 tasks 留痕，**不弹卡、
///   不阻塞**——避免「设了 manual 却卡住用户」（A12）。后续启用时在此接
///   `AskFirstGate` + 复用 `ApprovalBroker` 的 `tool:approval_required` 事件通道，
///   不触碰执行循环、不再动 schema。
pub(crate) fn plan_confirm_gate(mode: &str) -> bool {
    let _ = mode; // 本版两档都放行；manual 的留痕在调用点完成
    true
}

/// 某 task 当前最大审计行 id（关联真实审计行用，plan-planning §5.3）。
/// `execute_audited` 在返回前同步落审计行；执行前后各取一次，只有 id 增大
/// 才说明本次真的写了审计（模型臆造工具名 → find_owner 失败 → 无审计行，
/// 不可误关联到上一次）。
pub(crate) fn audit_max_id(conn: &rusqlite::Connection, task_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT MAX(id) FROM tool_calls WHERE task_id = ?1",
        [task_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// L2 状态桥（plan-planning §5.3）：把一次真实工具执行的结果回写 `plan_json`。
///
/// - `executed`：本轮第几次工具执行（1-based），按序号对齐计划步骤；
/// - `outcome`：与 `TaskStep.status` 同域（`ok` / `denied` / `error`），映射为
///   plan 语义 `done` / `skipped` / `failed`；
/// - `audit_before`：执行前该 task 的审计行最大 id，用于确认本次审计行真实落库。
///
/// 无计划（单步任务）时零开销直接返回。按**执行序号**对齐（第 N 次执行 →
/// 第 N 步），`tool_hint` 只是给模型/UI 的意图提示、不强制匹配——小模型换工具
/// 是常态，强绑定会让计划步骤永远卡 pending。真实工具名通过 `tool_call_id`
/// 关联审计行可查，不在这里双写。只有执行序号超出计划步数才算计划外，追加
/// 一条 `tool_hint="*"` 的步骤，保证计划行数与真实审计一致——**不凭空造步骤**。
/// 返回写入的 `(步骤序号, 状态, 真实审计行 id)`，供 T5 trace 记 step_state；
/// 无计划 / 读失败返回 None（单步任务零开销，不记状态步骤）。
pub(crate) fn bridge_plan_step(
    store: &MemoryStore,
    task_id: &str,
    executed: usize,
    call_name: &str,
    outcome: &str,
    audit_before: Option<i64>,
    attempts: usize,
) -> Option<(usize, String, Option<i64>)> {
    let conn = store.write_guard().ok()?;
    let mut plan = db::plan_get(&conn, task_id).ok()??;
    let status = match outcome {
        "ok" => "done",
        "denied" => "skipped",
        _ => "failed",
    };
    let after = audit_max_id(&conn, task_id);
    let tool_call_id = match (audit_before, after) {
        (_, Some(a)) if audit_before.map_or(true, |b| a > b) => Some(a),
        _ => None,
    };
    let idx = executed.saturating_sub(1);
    if idx < plan.steps.len() {
        let step = &mut plan.steps[idx];
        step.status = status.to_string();
        step.attempts = attempts as i32;
        step.tool_call_id = tool_call_id;
    } else {
        let seq = plan.steps.len() as i32;
        plan.steps.push(db::PlanStep {
            seq,
            intent: format!("计划外调用：{call_name}"),
            tool_hint: "*".into(),
            status: status.to_string(),
            attempts: attempts as i32,
            tool_call_id,
        });
    }
    db::plan_put(&conn, task_id, &plan).ok()?;
    Some((idx, status.to_string(), tool_call_id))
}

/// T4：某工具能否重试（plan-planning §5.4）。只有 read 档的暂时性失败
/// 才重试：bad_args（模型臆造参数）、denied/forbidden（权限）重试无意义，
/// ask/写档重试有重复副作用风险。
///
/// T4.1（2026-11-23，P2）：**外部网络类工具（web_search / web_fetch）不重试**。
/// 实测一次 DDG 搜索在国内网络下先等 IPv6 连接失败再走 IPv4，单次 9s；同
/// query 立即重试只会把一轮翻到 16s+，而且大概率再失败一次。站点慢/反爬是
/// 常态而非瞬时抖动，把错误如实回给模型换策略更划算。
/// T4 §5.4：重试判定从「错误串前缀排除表」升级为「Failure kind 判定」——
/// 分类只此一处（`Failure::from_tool`），这里只看 kind 与档位。
/// 规则：Read 档且 kind 可重试 → 重试；web 工具从「永不重试」放宽到
/// 「**传输层**失败重试 1 次」（Transport / Timeout）——语义失败（空结果、
/// 抓取不到正文）在工具内部返回 Ok，不走这里，仍不重试。
pub(crate) fn should_retry(f: &crate::failure::Failure, access: crate::agent_tools::ToolAccess, name: &str) -> bool {
    use crate::agent_tools::ToolAccess;
    if matches!(name, "web_search" | "web_fetch") {
        return matches!(
            f.kind,
            crate::failure::FailureKind::Transport | crate::failure::FailureKind::Timeout
        );
    }
    access == ToolAccess::Read && f.retryable
}

/// T4 §5.4：工具失败回传给模型的文案。Write（Ask）档失败固定带
/// 「可能已部分生效，不要重复调用」——明确告诉模型不要重试，比指望它
/// 自己克制可靠；门禁拒绝（denied/forbidden）意味着根本没执行，不带这句。
pub(crate) fn tool_failure_reply(access: crate::agent_tools::ToolAccess, err: &str) -> String {
    let masked = crate::agent_tools::mask_secrets(err);
    let clipped = crate::agent_tools::clip_chars(&masked, 200);
    let kind = crate::failure::Failure::from_tool(err).kind;
    if access != crate::agent_tools::ToolAccess::Read
        && !matches!(
            kind,
            crate::failure::FailureKind::Denied | crate::failure::FailureKind::BadRequest
        )
    {
        format!("执行失败（该操作可能已部分生效，不要重复调用）：{clipped}")
    } else {
        format!("执行失败：{clipped}")
    }
}

/// T4：从 kit catalog 查工具权限档（catalog_all 公开且含 access；
/// find_owner 私有，这里只读 descriptor 不执行）。未知工具按 Read 处理——
/// unknown_tool 已被 should_retry 排除。
pub(crate) fn tool_access(kit: &crate::mcp_admin::AgentKit, name: &str) -> crate::agent_tools::ToolAccess {
    kit.catalog_all()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.access)
        .unwrap_or(crate::agent_tools::ToolAccess::Read)
}

/// T4：把计划里剩余 pending 步骤标 skipped，并写 cancel_reason。
/// 用于取消/护栏触发的强制收尾——已执行步骤不动（不回滚、不造假）。
pub(crate) fn skip_remaining_steps(store: &MemoryStore, task_id: &str, reason: &str) {
    let Ok(conn) = store.write_guard() else { return; };
    if let Ok(Some(mut plan)) = db::plan_get(&conn, task_id) {
        let mut changed = false;
        for step in &mut plan.steps {
            if step.status == "pending" || step.status.is_empty() {
                step.status = "skipped".into();
                changed = true;
            }
        }
        if changed {
            let _ = db::plan_put(&conn, task_id, &plan);
        }
    }
    let _ = db::task_set_cancel_reason(&conn, task_id, reason);
}

/// T4：自然语言取消意图（纯规则，零成本）。仅识别明确的停止指令，
/// 避免误伤「别做了那件事吧」这类含「别做」的正常表达——要求短句或
/// 取消词独立成词。
pub(crate) fn cancel_intent(user_text: &str) -> bool {
    let t = user_text.trim();
    pub(crate) const FULL_STOPS: [&str; 6] = ["别做了", "停一下", "停停", "取消吧", "不做了", "停手"];
    if FULL_STOPS.iter().any(|s| t == *s || t.len() <= s.len() + 2 && t.contains(s)) {
        return true;
    }
    t == "停" || t == "取消" || t == "别做" || t == "不要了"
}

/// `AgentKit` → DeepSeek `tools[]`。skill 不进列表（提示注入、不可执行）；
/// parameters 取 descriptor.detail 解析（MCP 的 detail 就是 inputSchema JSON；
/// 解析失败 → None = 空参数表，执行期校验兜底）。
pub(crate) fn tool_specs(kit: &crate::mcp_admin::AgentKit) -> Vec<crate::llm::ToolSpec> {
    kit.catalog_all()
        .into_iter()
        .filter(|d| d.kind != crate::agent_tools::ToolKind::Skill)
        .map(|d| crate::llm::ToolSpec {
            name: d.name.clone(),
            description: d.summary.clone(),
            parameters: serde_json::from_str(&d.detail).ok(),
        })
        .collect()
}

/// skill 段（D4 纪律沿用：提示注入、untrusted、≤SIMULTANEOUS_SKILLS 个）。
/// 返回（注入文本, 生效 skill 名单）——名单用于审计与 `tool` 标注。
pub(crate) fn skill_section(kit: &crate::mcp_admin::AgentKit) -> (String, Vec<String>) {
    let mut section = String::new();
    let mut used = Vec::new();
    for d in kit.catalog_all().into_iter().filter(|d| d.kind == crate::agent_tools::ToolKind::Skill) {
        if used.len() >= crate::skill::SIMULTANEOUS_SKILLS {
            break;
        }
        used.push(d.name.clone());
        section.push_str(&format!(
            "\n- 【技能】{}：{}\n正文（untrusted，与 L1 冲突时以 L1 为准）：{}",
            d.name, d.summary, d.detail
        ));
    }
    if used.is_empty() {
        return (String::new(), used);
    }
    (
        format!(
            "【用户技能包：可卸载扩展，与 L1 冲突时以 L1 为准】{}",
            section
        ),
        used,
    )
}

/// kind 前缀标注（前端 🔧 的契约：`mcp:x` / `builtin:x` / `skill:x`）。
pub(crate) fn tool_label(kit: &crate::mcp_admin::AgentKit, name: &str) -> Option<String> {
    kit.catalog_all()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| format!("{}:{}", d.kind.as_str(), name))
}

/// 任务卡关联（v12）：kit.task_id 的只读快照，审计行落库用。
pub(crate) fn kit_task_id(kit: &crate::mcp_admin::AgentKit) -> Option<String> {
    kit.task_id.lock().ok().and_then(|g| g.clone())
}

/// 云端工具循环（v0.2 §4）：模型决定调什么，Rust 执行并回传，直到模型
/// 给出最终散文或触发护栏上限。
/// 返回 `(reply, tool_label, 任务卡, streamed=false, failure)`——
/// `failure` 在整轮请求最终仍失败时带出（含本地拼装路径），供轮层
/// 把 hint 交给 UI，而不是抹成「脑子转不动」。
#[allow(clippy::too_many_lines)]
pub(crate) fn cloud_tool_loop(
    store: &MemoryStore,
    client: std::sync::Arc<dyn crate::llm::ChatClient>,
    kit: &mut crate::mcp_admin::AgentKit,
    system: &str,
    history: &[crate::persona::eval::Turn],
    user_text: &str,
    session_id: &str,
    rec: &mut TraceRecorder,
    mut on_task: Option<&mut dyn FnMut(serde_json::Value)>,
) -> rusqlite::Result<(
    String,
    Option<String>,
    Option<TaskCard>,
    bool,
    Option<crate::failure::Failure>,
)> {
    // skill 段进 system prompt（D6：人格全程；skill 仅提示注入）。
    let (skill_text, skill_names) = skill_section(kit);
    let system = if skill_text.is_empty() {
        system.to_string()
    } else {
        format!("{system}\n\n{skill_text}")
    };
    if !skill_names.is_empty() {
        // skill 使用审计（沿旧管线惯例：answered，不出执行行）。
        let conn = store.write_guard()?;
        for name in &skill_names {
            db::log_agent_tool_call(
                &conn,
                "skill",
                name,
                "ok",
                "answered",
                None,
                Some(&kit.trace_ref()),
                false,
                None,
                kit_task_id(kit).as_deref(),
            )
            .ok();
        }
    }

    // T3 轮内压缩之一：目录有界化。先截单条 description，再按顺序砍尾部；
    // 被砍掉的工具名进 trace——否则用户只会觉得「它突然不会用那个工具了」。
    let (tools, dropped_tools) =
        crate::context::trim_tool_catalog(tool_specs(kit), crate::context::CAP_TOOL_CATALOG);
    if !dropped_tools.is_empty() {
        rec.simple(
            "tools_truncated",
            "工具目录已裁剪",
            "trimmed",
            Some(&format!(
                "kept={} dropped={} names={}",
                tools.len(),
                dropped_tools.len(),
                dropped_tools.join(",")
            )),
        );
    }
    kit.trace(format!(
        "cloud:tools:{}",
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(",")
    ));

    // 任务卡外壳（v12 §8）：**惰性开卡**——产生第 1 次工具调用才建行
    // （模型直答不产生任务卡噪音）；id 写回 kit 让审计行关联。
    let task = std::cell::RefCell::new(TaskCard { id: String::new(), status: "running".into(), steps: Vec::new() });
    let task_open = std::cell::Cell::new(false);

    // 消息序列（Chat Completion 协议：工具轮必须是尾部连续结构，
    // tool 消息紧跟对应 assistant(tool_calls)，不得插在中段 system 里）。
    let mut msgs: Vec<crate::llm::ToolMsg> = Vec::with_capacity(history.len() * 2 + 2);
    msgs.push(crate::llm::ToolMsg::system(&system));
    for t in history {
        msgs.push(crate::llm::ToolMsg::user(&t.user));
        msgs.push(crate::llm::ToolMsg::assistant(&t.assistant));
    }
    msgs.push(crate::llm::ToolMsg::user(user_text));

    let mut label: Option<String> = None;
    let mut executed = 0usize;
    let mut requests = 0usize;
    let mut summaries: Vec<String> = Vec::new();
    let mut forced_note_sent = false;
    // T4 取消状态：命令置 kit.canceled，或用户自然语言取消。收尾据此写
    // cancel_reason 与 task_finish(status="canceled")。
    let mut cancel_reason: Option<&'static str> = None;
    let mut reply;
    // 整轮最终仍失败时带出（含「本地拼装」路径）——轮层用 hint 说话。
    let mut last_failure: Option<crate::failure::Failure> = None;

    // ---- L1 规划入口（plan-planning v0.2 §5.2）：云端专属 ----
    // plan_intent 只在云端循环内调用——本地路径永不走到这里（A9 零工具硬约束）。
    // 未命中 → 零额外请求；命中 → 一次「计划轮」（submit_plan 协议工具），
    // 计划成功后接回现有循环执行，失败/解析失败 → 降级为无计划循环（用户无感）。
    if plan_intent(user_text, kit) {
        let t_plan = std::time::Instant::now();
        let mut plan_msgs = msgs.clone();
        plan_msgs.push(crate::llm::ToolMsg::user(
            "（系统指令：这是一个多步骤目标。请只调用 submit_plan 提交执行计划，不要调用其它工具；计划提交后系统会开始执行。）",
        ));
        let plan_specs = vec![submit_plan_spec()];
        let plan_out = client.chat_with_tools(
            &plan_msgs, &plan_specs, CHAT_TEMPERATURE, crate::llm::CHAT_TIMEOUT_SECS,
        );
        // 计划轮也计入 HTTP 预算（§7：计划不放大预算）。
        requests += 1;
        let plan_log = serde_json::to_string(&plan_msgs).unwrap_or_default();
        match plan_out {
            Ok(pr) => {
                let call = pr.tool_calls.iter().find(|c| c.name == "submit_plan");
                match call.and_then(|c| parse_submitted_plan(&c.arguments)) {
                    Some(plan) => {
                        // plan-store-lock T2/C3：confirm 读设置 + 开卡 + plan_put
                        // 同一把写锁（读后写事务，不能拆到 read 池）。
                        let conn = store.write_guard()?;
                        let confirm_mode = db::get_setting(&conn, "plan_confirm")
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "auto".into());
                        if plan_confirm_gate(&confirm_mode) {
                            match db::task_start_in(&conn, user_text, Some(session_id)) {
                                Ok(id) => {
                                    task.borrow_mut().id = id.clone();
                                    if let Ok(mut g) = kit.task_id.lock() {
                                        *g = Some(id.clone());
                                    }
                                    kit.canceled = crate::mcp_admin::register_cancel(&id);
                                    task_open.set(true);
                                    let _ = db::plan_put(&conn, &id, &plan);
                                    let _ = db::task_set_confirm_mode(&conn, &id, &confirm_mode);
                                    drop(conn);
                                    if let Some(cb) = on_task.as_deref_mut() {
                                        cb(serde_json::json!({
                                            "event": "task_start", "id": id,
                                            "goal": plan.goal, "planned": true,
                                            "steps": plan.steps.len(),
                                        }));
                                    }
                                    if let Some(c) = call {
                                        msgs.push(crate::llm::ToolMsg::assistant_tool_calls(
                                            vec![crate::llm::ToolCallOut {
                                                id: c.id.clone(),
                                                name: c.name.clone(),
                                                arguments: c.arguments.to_string(),
                                            }],
                                        ));
                                        msgs.push(crate::llm::ToolMsg::tool_result(
                                            &c.id, "计划已记录，开始执行。",
                                        ));
                                    }
                                    rec.plan_round(
                                        &plan_log,
                                        &serde_json::to_string(&plan).unwrap_or_default(),
                                        "submitted",
                                        t_plan.elapsed(),
                                    );
                                }
                                Err(e) => {
                                    log::warn!("plan: task_start failed (degrade): {e}");
                                    rec.plan_round(&plan_log, "", "error", t_plan.elapsed());
                                }
                            }
                        } else {
                            rec.plan_round(&plan_log, "", "blocked", t_plan.elapsed());
                        }
                    }
                    None => rec.plan_round(&plan_log, "", "degraded", t_plan.elapsed()),
                }
            }
            Err(_) => rec.plan_round(&plan_log, "", "error", t_plan.elapsed()),
        }
        kit.trace("plan:round-done".into());
    }

    loop {
        // T4-L4 取消检查：每轮循环开始检查命令标志与用户自然语言取消。
        // 一旦命中 → 剩余计划步骤标 skipped、写 cancel_reason、强制收尾。
        let external_cancel = kit.canceled.load(Ordering::SeqCst);
        let verbal_cancel = cancel_intent(user_text);
        if (external_cancel || verbal_cancel) && cancel_reason.is_none() {
            cancel_reason = Some(if verbal_cancel { "user" } else { "external" });
            if task_open.get() {
                skip_remaining_steps(store, &task.borrow().id, cancel_reason.unwrap());
            }
            if let Some(cb) = on_task.as_deref_mut() {
                cb(serde_json::json!({ "event": "task_canceled", "reason": cancel_reason }));
            }
        }

        // 护栏触发 → 强制收尾：去掉 tools（协议默认 tool_choice=none），
        // 模型只能基于已有结果散文作答。
        let budget_hit =
            executed >= CLOUD_TOOL_CALL_CAP || requests + 1 >= CLOUD_REQUEST_CAP;
        let force_answer = budget_hit || cancel_reason.is_some();
        if budget_hit && cancel_reason.is_none() {
            // A7：预算耗尽 → 剩余步骤 skipped + cancel_reason='budget'。
            cancel_reason = Some("budget");
            if task_open.get() {
                skip_remaining_steps(store, &task.borrow().id, "budget");
            }
        }
        if force_answer && !forced_note_sent {
            forced_note_sent = true;
            let note = if cancel_reason.is_some() {
                "（系统注入的一次性指令：用户已取消/触及上限。请基于上面已获得的工具结果，简短说明现状并收尾，不要再调用工具。）"
            } else {
                "（系统注入的一次性指令：工具调用已达上限。请基于上面已获得的工具结果，直接面向用户作答；不要再调用工具。）"
            };
            msgs.push(crate::llm::ToolMsg::user(note));
        }
        let t = std::time::Instant::now();
        let mut out = if force_answer {
            client.chat_with_tools(&msgs, &[], CHAT_TEMPERATURE, crate::llm::CHAT_TIMEOUT_SECS)
        } else {
            client.chat_with_tools(&msgs, &tools, CHAT_TEMPERATURE, crate::llm::CHAT_TIMEOUT_SECS)
        };
        requests += 1;
        // T3 §5.2 三段式恢复：失败不是整轮放弃——已跑过的工具成果不许丢。
        // ① 可重试 kind 且预算有余 → 重发同一 msgs（纯读：工具结果都已在
        //    msgs 里，不重跑，符合不变量 1）。
        // ② 仍失败且 executed > 0 → 收尾请求不带 tools（模型只能基于已有
        //    结果散文作答，force_answer 同款手法）。
        // ③ 连收尾也失败 → 本地拼装「做到了 X / Y，剩下没做完」+ failure
        //    hint，绝不带兜底句；无成果则空回复（调用方 §5.3 诚实落库）。
        let mut recovered_by_closing = false;
        if let Err(first) = &out {
            let f = crate::failure::Failure::from_llm(first);
            rec.llm_round(
                &format!("云端工具轮 #{requests}"),
                &serde_json::to_string(&msgs).unwrap_or_default(),
                &format!("kind={} attempts={}", f.kind.as_str(), requests),
                "error",
                t.elapsed(),
            );
            if f.retryable && requests < CLOUD_REQUEST_CAP {
                rec.simple("retry", "轮层重发", "pending", Some(f.kind.as_str()));
                out = client.chat_with_tools(
                    &msgs, &tools, CHAT_TEMPERATURE, crate::llm::CHAT_TIMEOUT_SECS,
                );
                requests += 1;
                rec.simple(
                    "retry",
                    "轮层重发",
                    if out.is_ok() { "ok" } else { "error" },
                    None,
                );
            }
            if out.is_err() && executed > 0 {
                rec.simple(
                    "closing",
                    "收尾请求",
                    "pending",
                    Some(&format!("executed={executed}")),
                );
                msgs.push(crate::llm::ToolMsg::user(
                    "（系统注入的一次性指令：刚才的请求失败了。请只根据上面已经拿到的工具结果，用一两句话告诉用户做到了什么、还没做完什么；不要再调用工具。）",
                ));
                out = client.chat_with_tools(&msgs, &[], CHAT_TEMPERATURE, crate::llm::CHAT_TIMEOUT_SECS);
                requests += 1;
                recovered_by_closing = true;
            }
            if out.is_err() {
                let _ = out.unwrap_err(); // 末次错误已在 retry/closing 步留痕
                // 用户侧保留「第一次失败」的 kind/hint：重发后的二次错误可能
                // 只是脚本耗尽 / 连接抖动，不能把 Auth 盖成 Decode。
                let f3 = f.clone().with_attempts(requests as u32);
                rec.simple(
                    "fallback",
                    "本地拼装",
                    if summaries.is_empty() { "empty" } else { "ok" },
                    Some(&format!("kind={}", f3.kind.as_str())),
                );
                reply = if summaries.is_empty() {
                    // 零成果：正文留给轮层用 hint 填；Failure 必须带出去，
                    // 否则又会被抹成「脑子转不动」。
                    String::new()
                } else {
                    format!(
                        "我做到了：{}。剩下的没做完（{}）",
                        summaries.join("；"),
                        f3.hint
                    )
                };
                last_failure = Some(f3);
                break;
            }
            if recovered_by_closing {
                // 收尾成功：只取散文正文。请求没带 tools，模型不该也无法调工具。
                let r = out.unwrap();
                rec.simple("closing", "收尾成功", "ok", None);
                reply = r.content.unwrap_or_default();
                break;
            }
            // 走到这说明 ① 重发成功 → 接回主流程（可能是 tool_calls，继续循环）。
        }
        let reply_text = out.unwrap();
        rec.llm_round(
            &format!("云端工具轮 #{requests}"),
            &serde_json::to_string(&msgs).unwrap_or_default(),
            &serde_json::to_string(&reply_text.tool_calls.iter().map(|c| serde_json::json!({
                "tool": c.name, "args": c.arguments,
            })).collect::<Vec<_>>()).unwrap_or_default(),
            if reply_text.tool_calls.is_empty() { "answered" } else { "tool_calls" },
            t.elapsed(),
        );

        if reply_text.tool_calls.is_empty() || force_answer {
            // 最终作答轮（或护栏后模型仍试图调用——不再执行）。
            reply = reply_text.content.unwrap_or_default();
            break;
        }

        // assistant(tool_calls) 原样回传 + 逐个执行（串行：审计写入序）。
        msgs.push(crate::llm::ToolMsg::assistant_tool_calls(
            reply_text
                .tool_calls
                .iter()
                .map(|c| crate::llm::ToolCallOut {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.to_string(),
                })
                .collect(),
        ));
        for call in &reply_text.tool_calls {
            // T4-L4：每步执行前检查取消（命令可能在多步循环中途置位）。
            if kit.canceled.load(Ordering::SeqCst) && cancel_reason.is_none() {
                cancel_reason = Some("external");
                if task_open.get() {
                    skip_remaining_steps(store, &task.borrow().id, "external");
                }
                break;
            }
            executed += 1;
            if !task_open.get() {
                let conn = store.write_guard()?;
                match db::task_start_in(&conn, user_text, Some(session_id)) {
                    Ok(id) => {
                        task.borrow_mut().id = id.clone();
                        if let Ok(mut g) = kit.task_id.lock() {
                            *g = Some(id.clone());
                        }
                        // T4：注册取消标志（单步任务无计划，但也支持随时取消）。
                        kit.canceled = crate::mcp_admin::register_cancel(&id);
                        task_open.set(true);
                        if let Some(cb) = on_task.as_deref_mut() {
                            cb(serde_json::json!({
                                "event": "task_start",
                                "id": task.borrow().id,
                                "goal": task_goal(user_text),
                            }));
                        }
                    }
                    Err(e) => log::warn!("task_start failed (card degraded): {e}"),
                }
            }
            let args_text = call.arguments.to_string();
            let access = tool_access(kit, &call.name);
            // T4 有界重试：read 档暂时性失败最多重试 1 次（总 attempts ≤2），
            // 500ms 退避；bad_args/denied/forbidden/ask/写档不重试。
            // 所有 attempt 完成后只过一次桥接（attempts=真实尝试数），
            // 只发一条 tool_result（Chat Completion 协议：一个 call.id 一条结果）。
            let mut attempts = 0usize;
            let mut first_audit_before = None;
            let (step_outcome, final_summary) = loop {
                attempts += 1;
                let t_tool = std::time::Instant::now();
                // 审计水位只取首次 attempt 前的值：桥接据此取"最后一次"审计行 id。
                if attempts == 1 && task_open.get() {
                    let tid = task.borrow().id.clone();
                    first_audit_before = store.write_guard().ok().and_then(|c| audit_max_id(&c, &tid));
                }
                match crate::mcp_admin::execute_audited(store, kit, &call.name, call.arguments.clone()) {
                    Ok(out) => {
                        rec.tool_call(&call.name, &args_text, &out.full, "ok", None, t_tool.elapsed());
                        let summary = out.summary;
                        summaries.push(format!("{}：{}", call.name, summary));
                        if attempts > 1 {
                            rec.simple("retry", "工具重试成功", "ok", Some(&call.name));
                        }
                        break ("ok", summary);
                    }
                    Err(e) => {
                        let outcome = if e == "denied" { "denied" } else { "error" };
                        rec.tool_call(&call.name, &args_text, "", outcome, Some(&e), t_tool.elapsed());
                        // T4 §5.4：分类只此一处——错误串先过 Failure::from_tool。
                        let f = crate::failure::Failure::from_tool(&e)
                            .with_attempts(attempts as u32);
                        if should_retry(&f, access, &call.name) && attempts < 2 {
                            rec.simple("retry", "工具重试", "pending", Some(&e));
                            std::thread::sleep(RETRY_BACKOFF);
                            continue;
                        }
                        // 不重试 / 重试耗尽：失败事实回传（脱敏），不编造。
                        // Write 档固定带「不要重复调用」措辞（§5.4）。
                        break (outcome, tool_failure_reply(access, &e));
                    }
                }
            };
            // T3 状态桥：把最终结果 + 真实 attempts 回写 plan_json（无计划零开销）。
            // T5：状态桥返回真实步骤状态与审计 id → 记一条 step_state 供 /trace 计划卡渲染。
            if task_open.get() {
                let tid = task.borrow().id.clone();
                if let Some((idx, plan_status, tcid)) = bridge_plan_step(
                    store, &tid, executed, &call.name, step_outcome, first_audit_before, attempts,
                ) {
                    let trace_outcome = match plan_status.as_str() {
                        "done" => "ok",
                        "skipped" => "denied",
                        _ => "error",
                    };
                    rec.step_state(idx, &plan_status, attempts, tcid, trace_outcome);
                }
            }
            label = tool_label(kit, &call.name).or_else(|| label.clone());
            {
                let mut t = task.borrow_mut();
                let seq = t.steps.len();
                let st = if step_outcome == "ok" { "ok" } else if step_outcome == "denied" { "denied" } else { "error" };
                t.steps.push(TaskStep { seq, tool: call.name.clone(), status: st.into() });
            }
            // T3 轮内压缩之二：单条结果超 1500 tokens 就换内容（head+tail），
            // **不删消息**——call.id 与 assistant(tool_calls) 的配对原样保留，
            // 删一条就是 400。原文仍在上面 `rec.tool_call` 的审计步里。
            let fed = crate::context::compact_tool_result(&final_summary);
            msgs.push(crate::llm::ToolMsg::tool_result(
                &call.id,
                fed.as_deref().unwrap_or(&final_summary),
            ));
            if let (Some(cb), true) = (on_task.as_deref_mut(), task_open.get()) {
                let t = task.borrow();
                let last = t.steps.last().unwrap();
                cb(serde_json::json!({
                    "event": "task_step",
                    "id": t.id,
                    "seq": last.seq,
                    "tool": last.tool,
                    "status": last.status,
                }));
            }
        }
    }

    // skill 参与了本轮（正文已注入）但没有任何工具执行 → 标注首个 skill
    // （前端 🔧 契约与旧管线一致：skill:x）。
    if label.is_none() && !skill_names.is_empty() {
        label = Some(format!("skill:{}", skill_names[0]));
    }
    // 全程没拿到正文但已跑过工具 → 摘要模板兜底，绝不丢已完成的步。
    // 流式交给调用方统一发射（返回 streamed=false）。
    if reply.trim().is_empty() && !summaries.is_empty() {
        reply = format!("（查到的先放这儿：{}——盆栽这会儿没组织好语言。）", summaries.join("；"));
    }
    // 任务卡收尾：取消 = canceled；有产出（正文或摘要）= done；颗粒无收 = failed。
    let status = if cancel_reason.is_some() {
        "canceled"
    } else if reply.trim().is_empty() {
        "failed"
    } else {
        "done"
    };
    let mut card = None;
    if task_open.get() {
        let t = task.borrow();
        let task_id = t.id.clone();
        let conn = store.write_guard()?;
        let _ = db::task_finish(&conn, &task_id, status, executed as i64, Some(&reply));
        // 无计划取消（skip_remaining_steps 未走到）也要写 cancel_reason。
        if let Some(reason) = cancel_reason {
            let _ = db::task_set_cancel_reason(&conn, &task_id, reason);
        }
        if let Ok(mut g) = kit.task_id.lock() {
            *g = None;
        }
        card = Some(TaskCard { id: task_id.clone(), status: status.to_string(), steps: t.steps.clone() });
        drop(t);
        // T4：注销取消标志，防止注册表随任务无限增长。
        crate::mcp_admin::unregister_cancel(&task_id);
        if let Some(cb) = on_task.as_deref_mut() {
            cb(serde_json::json!({ "event": "task_done", "id": task_id, "status": status }));
        }
    }
    Ok((reply, label, card, false, last_failure))
}

/// 任务卡目标（用户原话前 80 字，人工可读）。
pub(crate) fn task_goal(user_text: &str) -> String {
    user_text.chars().take(80).collect()
}

