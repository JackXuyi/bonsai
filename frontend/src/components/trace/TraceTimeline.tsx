"use client";

// 链路追踪时间线（spec-s8 S8-d）：trace_steps 按 seq 渲染垂直时间线。
// 节点色按 step_kind 分组；llm_round / tool_call 可展开看全文。
import { useState } from "react";
import type { TraceStep } from "@/lib/types";
import { TraceStepCard } from "./TraceStepCard";

const STEP_TONE: Record<TraceStep["stepKind"], string> = {
  crisis_check: "tone-neutral",
  tool_intent: "tone-neutral",
  llm_round: "tone-llm",
  tool_call: "tone-tool",
  filter: "tone-ok",
  json_retry: "tone-warn",
  prose_retry: "tone-warn",
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
    Boolean((step.stepKind === "json_retry" || step.stepKind === "prose_retry") && step.output);

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

/** 展开区：llm_round 看 prompt/output 全文；tool_call 看参数与返回摘要。 */
function TraceStepDetail({ step }: { step: TraceStep }) {
  return (
    <div className="trace-detail">
      {step.userText && <PreBlock label="当前输入（user message，不在 system prompt 里）" text={step.userText} />}
      {step.prompt && <PreBlock label="本轮 prompt（全文）" text={step.prompt} />}
      {step.output && <PreBlock label="模型原始输出" text={step.output} />}
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
  lines?: { ts?: string; who?: string; text?: string }[];
  facts?: string[];
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
  return (
    <div className="trace-tool-result">
      <div className="trace-tool-head">
        <span className="trace-tool-window">{data.window}</span>
        <span className="trace-tool-count">
          对话 {data.dialogue_count ?? lines.length} 条 · 记忆 {data.fact_count ?? facts.length} 条
        </span>
      </div>
      {lines.length === 0 && facts.length === 0 && (
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
