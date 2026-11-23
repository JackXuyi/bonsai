"use client";

// 链路追踪时间线（spec-s8 S8-d）：trace_steps 按 seq 渲染垂直时间线。
// 节点色按 step_kind 分组；llm_round / tool_call 可展开看全文。
// prompt 数组结构化渲染（2026-11-22）：云端循环的 prompt 是 JSON 序列化的
// ToolMsg[]，按角色分段 + markdown 渲染；本地路径纯文本 / 坏 JSON 回落原样。
import { useState } from "react";
import { renderMarkdown } from "@/lib/markdown";
import type { TraceStep } from "@/lib/types";
import { TraceStepCard } from "./TraceStepCard";

const STEP_TONE: Record<TraceStep["stepKind"], string> = {
  user_input: "tone-ok",
  crisis_check: "tone-neutral",
  tool_intent: "tone-neutral",
  summary_intent: "tone-neutral",
  context_budget: "tone-neutral",
  tools_truncated: "tone-warn",
  llm_round: "tone-llm",
  tool_call: "tone-tool",
  filter: "tone-ok",
  json_retry: "tone-warn",
  prose_retry: "tone-warn",
  plan_submit: "tone-plan",
  step_state: "tone-plan",
  retry: "tone-warn",
};

/** 失败/重试类 outcome 覆盖默认节点色（denied/error → 红）。 */
function toneFor(step: TraceStep): string {
  if (step.outcome === "denied" || step.outcome === "error") return "tone-bad";
  return STEP_TONE[step.stepKind] ?? "tone-neutral";
}

function outcomeText(step: TraceStep): string {
  const map: Record<string, string> = {
    ok: "成功",
    hit: "命中",
    miss: "未命中",
    pass: "通过",
    answered: "有输出",
    direct: "直答",
    none: "无输出",
    trimmed: "已压缩",
    denied: "被拒绝",
    error: "出错",
  };
  return step.outcome ? (map[step.outcome] ?? step.outcome) : "—";
}

export function TraceTimeline({ steps }: { steps: TraceStep[] }) {
  return (
    <div className="trace-timeline">
      {steps.map((step) => (
        <TraceNode key={step.seq} step={step} />
      ))}
    </div>
  );
}

function TraceNode({ step }: { step: TraceStep }) {
  const [open, setOpen] = useState(false);
  const expandable =
    step.stepKind === "llm_round" ||
    step.stepKind === "tool_call" ||
    step.stepKind === "user_input" ||
    step.stepKind === "plan_submit" ||
    step.stepKind === "step_state" ||
    step.stepKind === "context_budget" ||
    Boolean(
      (step.stepKind === "json_retry" ||
        step.stepKind === "prose_retry" ||
        step.stepKind === "retry") &&
        step.output
    );

  return (
    <div className="trace-node">
      <span className={`trace-dot ${toneFor(step)}`} aria-hidden="true" />
      <div className="trace-node-body">
        <TraceStepCard
          step={step}
          outcomeText={outcomeText(step)}
          expandable={expandable}
          open={open}
          onToggle={() => setOpen((v) => !v)}
        />
        {open && <TraceStepDetail step={step} />}
      </div>
    </div>
  );
}

/** 展开区：user_input 看输入全文；llm_round 看 prompt/output 全文；
 * tool_call 看参数与返回摘要；plan_submit / step_state 看计划卡。 */
function TraceStepDetail({ step }: { step: TraceStep }) {
  const isPlan = step.stepKind === "plan_submit" || step.stepKind === "step_state";
  return (
    <div className="trace-detail">
      {step.userText && step.stepKind !== "user_input" && (
        <PreBlock label="当前输入（user message，不在 system prompt 里）" text={step.userText} />
      )}
      {step.stepKind === "user_input" && step.output && (
        <PreBlock label="对话输入全文" text={step.output} />
      )}
      {step.prompt && <PromptBlock label="本轮 prompt（全文）" text={step.prompt} />}
      {step.stepKind === "plan_submit" && step.output && (
        <PlanSubmitBlock text={step.output} />
      )}
      {step.stepKind === "step_state" && step.output && (
        <PlanStateBlock text={step.output} />
      )}
      {step.stepKind === "context_budget" && step.output && (
        <PreBlock label="本轮预算（估算 / 可用 / 窗口 / 丢弃轮数）" text={step.output} />
      )}
      {step.output && !isPlan && step.stepKind !== "user_input" && step.stepKind !== "context_budget" && (
        <PreBlock label="模型原始输出" text={step.output} />
      )}
      {step.argsJson && <PreBlock label="工具请求参数" text={step.argsJson} />}
      {step.summary && step.stepKind === "tool_call" && <ToolResultBlock text={step.summary} />}
      {step.summary && step.stepKind !== "tool_call" && <PreBlock label="工具返回摘要" text={step.summary} />}
    </div>
  );
}

// ---------------------------------------------------------------- 工具返回结构化渲染

/** memory_search 的 JSON 形态（2026-11-21 裁决：工具 trace 数据一律 JSON）。
 * 结构化渲染只认这个形状；其他 JSON pretty 打印，坏 JSON 原样兜底。 */
interface MemoryResultData {
  window: string;
  dialogue_count?: number;
  fact_count?: number;
  /** S1 三级降级：关键词腿全落空 → 退回整窗（结果可能不完整）。 */
  degraded?: boolean;
  lines?: { ts?: string; who?: string; text?: string }[];
  facts?: string[];
  /** S2 跨时间腿：窗口外按相关度召回的长期记忆（事实/偏好/边界）。 */
  beyond?: { tag?: string; text?: string }[];
}

/** 工具返回摘要：认识 memory_search 形态 → 结构化卡片；其他 JSON →
 * pretty 打印；解析失败 → 原样文本。导出以便单测。 */
export function ToolResultBlock({ text }: { text: string }) {
  try {
    const j = JSON.parse(text) as unknown;
    if (
      j &&
      typeof j === "object" &&
      typeof (j as { window?: unknown }).window === "string" &&
      Array.isArray((j as { lines?: unknown }).lines)
    ) {
      return <MemoryResultCard data={j as MemoryResultData} />;
    }
    if (j && typeof j === "object") {
      return <PreBlock label="工具返回（JSON）" text={JSON.stringify(j, null, 2)} />;
    }
  } catch {
    // 非 JSON → 原样兜底
  }
  return <PreBlock label="工具返回" text={text} />;
}

function MemoryResultCard({ data }: { data: MemoryResultData }) {
  const lines = data.lines ?? [];
  const facts = data.facts ?? [];
  const beyond = data.beyond ?? [];
  return (
    <div className="trace-tool-result">
      <div className="trace-tool-head">
        <span className="trace-tool-window">{data.window}</span>
        <span className="trace-tool-count">
          对话 {data.dialogue_count ?? lines.length} 条 · 记忆 {data.fact_count ?? facts.length} 条
        </span>
        {data.degraded && <span className="trace-tool-degraded">关键词未命中 · 已退回整窗</span>}
      </div>
      {lines.length === 0 && facts.length === 0 && beyond.length === 0 && (
        <p className="trace-tool-empty">没有找到对话记录。</p>
      )}
      {lines.length > 0 && (
        <ul className="trace-tool-lines">
          {lines.map((l, i) => (
            <li key={`${l.ts ?? ""}-${i}`} className={l.who === "用户" ? "from-user" : "from-agent"}>
              <span className="trace-tool-ts">{l.ts ?? ""}</span>
              <span className="trace-tool-who">{l.who ?? "—"}</span>
              <span className="trace-tool-text">{l.text ?? ""}</span>
            </li>
          ))}
        </ul>
      )}
      {facts.length > 0 && (
        <div className="trace-tool-facts">
          <span className="trace-tool-facts-label">记忆</span>
          <ul className="trace-tool-facts-list">
            {facts.map((f, i) => (
              <li key={i}>{f}</li>
            ))}
          </ul>
        </div>
      )}
      {beyond.length > 0 && (
        <div className="trace-tool-facts trace-tool-beyond">
          <span className="trace-tool-facts-label">更早的记忆（窗口外）</span>
          <ul className="trace-tool-facts-list">
            {beyond.map((b, i) => (
              <li key={i}>
                {b.tag && <span className="trace-tool-ts">{b.tag}</span>}
                {b.text ?? ""}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function PreBlock({ label, text }: { label: string; text: string }) {
  const copy = () => {
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text).then(
        () => {},
        () => {}
      );
    }
  };
  return (
    <div className="trace-pre">
      <div className="trace-pre-head">
        <span className="trace-pre-label">{label}</span>
        <button type="button" className="msg-act" onClick={copy}>
          复制
        </button>
      </div>
      <pre>{text}</pre>
    </div>
  );
}

// ---------------------------------------------------------------- prompt 数组结构化渲染

/** prompt 消息条目（cloud_tool_loop 的 llm_round：`serde_json::to_string(&ToolMsg[])`）。
 * Rust 侧 skip_serializing_if：assistant 工具调用声明消息**没有 content 字段**
 * （content: None），tool 消息带 tool_call_id——这些字段都必须当可选处理，
 * 否则只要模型调过一次工具，整条 prompt 就解析失败回落原始 JSON
 * （2026-11-23 渲染 bug 的根因）。 */
interface PromptCall {
  id?: string;
  function?: { name?: string; arguments?: string };
}

interface PromptMsg {
  role: string;
  content?: string | null;
  tool_calls?: PromptCall[];
  tool_call_id?: string;
}

const ROLE_LABEL: Record<string, string> = {
  system: "系统",
  user: "用户",
  assistant: "助手",
  tool: "工具返回",
};

/** 宽松解析 ToolMsg[]：合法的 {role, content?, tool_calls?} 对象数组才走
 * 结构化；其余 null。content 可缺失/null（assistant_tool_calls 形态）。 */
export function parsePromptMessages(text: string): PromptMsg[] | null {
  try {
    const j = JSON.parse(text) as unknown;
    if (!Array.isArray(j) || j.length === 0) return null;
    const ok = j.every((m) => {
      if (!m || typeof m !== "object") return false;
      const msg = m as Record<string, unknown>;
      if (typeof msg.role !== "string") return false;
      if (msg.content !== undefined && msg.content !== null && typeof msg.content !== "string") return false;
      if (msg.tool_calls !== undefined && !Array.isArray(msg.tool_calls)) return false;
      return true;
    });
    return ok ? (j as PromptMsg[]) : null;
  } catch {
    return null;
  }
}

/** prompt 展示块：数组形态按角色分段、正文 markdown 渲染（renderMarkdown
 * 先转义原文，无注入面扩大）；本地路径纯文本 / 坏 JSON 回落 PreBlock 原样。
 * 复制按钮始终复制原始全文（排障语义）。导出以便单测。 */
export function PromptBlock({ label, text }: { label: string; text: string }) {
  const copy = () => {
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text).then(
        () => {},
        () => {}
      );
    }
  };
  const msgs = parsePromptMessages(text);
  if (!msgs) return <PreBlock label={label} text={text} />;
  return (
    <div className="trace-pre">
      <div className="trace-pre-head">
        <span className="trace-pre-label">{label}</span>
        <button type="button" className="msg-act" onClick={copy}>
          复制
        </button>
      </div>
      <ol className="trace-prompt">
        {msgs.map((m, i) => (
          <li key={i} className={`trace-prompt-msg role-${m.role}`}>
            <div className="trace-prompt-role">
              <span className={`trace-prompt-badge role-${m.role}`}>
                {ROLE_LABEL[m.role] ?? m.role}
              </span>
              {m.tool_call_id && <span className="trace-prompt-index">id: {m.tool_call_id}</span>}
              <span className="trace-prompt-index">
                {i + 1}/{msgs.length}
              </span>
            </div>
            {m.content && (
              <div
                className="trace-prompt-body"
                dangerouslySetInnerHTML={{ __html: renderMarkdown(m.content) }}
              />
            )}
            {m.tool_calls && m.tool_calls.length > 0 && (
              <ul className="trace-prompt-calls">
                {m.tool_calls.map((c, ci) => (
                  <li key={c.id ?? ci} className="trace-prompt-call">
                    <span className="trace-prompt-call-name">{c.function?.name ?? "未知工具"}</span>
                    <pre className="trace-prompt-call-args">{prettyArgs(c.function?.arguments)}</pre>
                  </li>
                ))}
              </ul>
            )}
            {!m.content && (!m.tool_calls || m.tool_calls.length === 0) && (
              <div className="trace-prompt-body trace-prompt-empty">（无正文）</div>
            )}
          </li>
        ))}
      </ol>
    </div>
  );
}

/** 协议里 arguments 是 JSON 字符串——pretty 打印；解析失败原样。 */
function prettyArgs(args?: string): string {
  if (!args) return "{}";
  try {
    return JSON.stringify(JSON.parse(args), null, 2);
  } catch {
    return args;
  }
}

// ---------------------------------------------------------------- 计划卡（T5）

/** 后端 PlanJson 的宽松镜像（serde snake_case 落库契约）。字段一律可选，
 * 坏 JSON / 老版本缺字段都不炸——解析失败返回 null 由调用方回落原文。 */
export interface PlanStepView {
  seq?: number;
  intent?: string;
  tool_hint?: string;
  status?: string;
  attempts?: number;
  tool_call_id?: number | null;
}

export interface PlanSubmitView {
  goal?: string;
  plan_mode?: string;
  steps?: PlanStepView[];
}

export interface PlanStateView {
  seq?: number;
  status?: string;
  attempts?: number;
  tool_call_id?: number | null;
}

/** 解析计划轮 output（完整 PlanJson）。只认 object + steps 数组，否则 null。 */
export function parseSubmittedPlan(text: string): PlanSubmitView | null {
  try {
    const j = JSON.parse(text) as unknown;
    if (!j || typeof j !== "object") return null;
    const o = j as Record<string, unknown>;
    if (typeof o.goal !== "string" || !Array.isArray(o.steps)) return null;
    return o as unknown as PlanSubmitView;
  } catch {
    return null;
  }
}

/** 解析步骤状态 output（{seq,status,attempts,tool_call_id}）。 */
export function parsePlanState(text: string): PlanStateView | null {
  try {
    const j = JSON.parse(text) as unknown;
    if (!j || typeof j !== "object") return null;
    const o = j as Record<string, unknown>;
    if (typeof o.status !== "string") return null;
    return o as unknown as PlanStateView;
  } catch {
    return null;
  }
}

const PLAN_STATUS_LABEL: Record<string, string> = {
  pending: "待执行",
  running: "执行中",
  done: "完成",
  failed: "失败",
  skipped: "跳过",
};

function planStatusText(status?: string): string {
  return status ? (PLAN_STATUS_LABEL[status] ?? status) : "—";
}

/** 计划提交卡：目标 + 步骤列表（意图 / 工具 / 状态 / 关联审计 id）。 */
export function PlanSubmitBlock({ text }: { text: string }) {
  const plan = parseSubmittedPlan(text);
  if (!plan) return <PreBlock label="计划（原文）" text={text} />;
  return (
    <div className="trace-plan">
      <div className="trace-plan-head">
        <span className="trace-plan-badge">计划</span>
        <span className="trace-plan-goal">{plan.goal}</span>
        {plan.plan_mode && <span className="trace-plan-mode">模式 {plan.plan_mode}</span>}
      </div>
      <ol className="trace-plan-steps">
        {(plan.steps ?? []).map((st, i) => (
          <li key={st.seq ?? i} className={`trace-plan-step status-${st.status ?? "pending"}`}>
            <span className="trace-plan-seq">{st.seq != null ? st.seq + 1 : i + 1}</span>
            <span className="trace-plan-intent">{st.intent ?? "—"}</span>
            {st.tool_hint && st.tool_hint !== "*" && (
              <span className="trace-plan-tool">{st.tool_hint}</span>
            )}
            <span className="trace-plan-status">{planStatusText(st.status)}</span>
            {st.tool_call_id != null && (
              <span className="trace-plan-audit">审计 #{st.tool_call_id}</span>
            )}
          </li>
        ))}
      </ol>
    </div>
  );
}

/** 步骤状态卡：单步执行结果（状态 / 尝试次数 / 关联审计 id）。 */
export function PlanStateBlock({ text }: { text: string }) {
  const st = parsePlanState(text);
  if (!st) return <PreBlock label="步骤状态（原文）" text={text} />;
  return (
    <div className={`trace-plan-state status-${st.status ?? "pending"}`}>
      <span className="trace-plan-badge">步骤 {st.seq != null ? st.seq + 1 : "?"}</span>
      <span className="trace-plan-status">{planStatusText(st.status)}</span>
      {st.attempts != null && st.attempts > 1 && (
        <span className="trace-plan-attempts">尝试 {st.attempts} 次</span>
      )}
      {st.tool_call_id != null && <span className="trace-plan-audit">审计 #{st.tool_call_id}</span>}
    </div>
  );
}
