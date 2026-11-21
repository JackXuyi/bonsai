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
      {step.prompt && <PreBlock label="本轮 prompt（全文）" text={step.prompt} />}
      {step.output && <PreBlock label="模型原始输出" text={step.output} />}
      {step.argsJson && <PreBlock label="工具请求参数" text={step.argsJson} />}
      {step.summary && <PreBlock label="工具返回摘要" text={step.summary} />}
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
