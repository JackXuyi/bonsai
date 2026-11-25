"use client";

// 时间线单步卡（spec-s8 S8-d）：标题 + outcome/耗时行，可展开标记。
import type { TraceStep } from "@/lib/types";
import { toolArgsPreview } from "@/lib/formatToolArgs";

/** 折叠态摘要：计划卡显示目标，步骤卡显示状态。坏 JSON 返回 null 不显示。 */
function planHint(output?: string | null): string | null {
  if (!output) return null;
  try {
    const j = JSON.parse(output) as { goal?: unknown };
    return typeof j.goal === "string" ? j.goal.slice(0, 120) : null;
  } catch {
    return null;
  }
}

function stateHint(output?: string | null): string | null {
  if (!output) return null;
  try {
    const j = JSON.parse(output) as { seq?: unknown; status?: unknown; tool_call_id?: unknown };
    const seq = typeof j.seq === "number" ? `步骤 ${j.seq + 1}` : "步骤";
    const status = typeof j.status === "string" ? j.status : "?";
    const audit = typeof j.tool_call_id === "number" ? ` · 审计 #${j.tool_call_id}` : "";
    return `${seq} → ${status}${audit}`;
  } catch {
    return null;
  }
}

export function TraceStepCard({
  step,
  outcomeText,
  expandable,
  open,
  onToggle,
}: {
  step: TraceStep;
  outcomeText: string;
  expandable: boolean;
  open: boolean;
  onToggle: () => void;
}) {
  const firstOutput = step.output?.split("\n")[0]?.slice(0, 120);
  return (
    <div className={`trace-card${expandable ? " expandable" : ""}`} onClick={expandable ? onToggle : undefined}>
      <div className="trace-card-head">
        <span className="trace-title">
          {step.title}
          {expandable && <span className="trace-caret">{open ? " ▾" : " ▸"}</span>}
        </span>
        <span className="trace-meta">
          {step.durationMs != null && <span>{step.durationMs >= 1 ? `${Math.round(step.durationMs)}ms` : "<1ms"}</span>}
          <span>{outcomeText}</span>
        </span>
      </div>
      {step.stepKind === "llm_round" && firstOutput && !open && (
        <p className="trace-hint">输出：{firstOutput}</p>
      )}
      {step.stepKind === "user_input" && !open && (
        <p className="trace-hint">输入：{step.userText?.slice(0, 120)}</p>
      )}
      {step.stepKind === "tool_call" && !open && step.argsJson && (
        <p className="trace-hint">参数：{toolArgsPreview(step.argsJson)}</p>
      )}
      {step.stepKind === "plan_submit" && !open && planHint(step.output) && (
        <p className="trace-hint">目标：{planHint(step.output)}</p>
      )}
      {step.stepKind === "step_state" && !open && stateHint(step.output) && (
        <p className="trace-hint">{stateHint(step.output)}</p>
      )}
    </div>
  );
}
