"use client";

// 时间线单步卡（spec-s8 S8-d）：标题 + outcome/耗时行，可展开标记。
import type { TraceStep } from "@/lib/types";

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
      {step.stepKind === "tool_call" && !open && step.argsJson && (
        <p className="trace-hint">参数：{step.argsJson.slice(0, 120)}</p>
      )}
    </div>
  );
}
