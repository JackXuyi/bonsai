"use client";

// 计划步骤列表 + 恢复操作（从 PlanProgressCard 抽出，聊天 inline 复用）。
import { useState } from "react";
import { taskReentryChoice, taskResume } from "@/lib/commands";
import { usePlanStore } from "@/lib/stores/planStore";
import type { PlanProgress, PlanStepStatus } from "@/lib/types";

const STEP_LABEL: Record<PlanStepStatus, string> = {
  pending: "待办",
  running: "进行中",
  done: "完成",
  failed: "失败",
  skipped: "跳过",
};

const TASK_LABEL: Record<string, string> = {
  running: "进行中",
  done: "已完成",
  failed: "失败",
  canceled: "已取消",
  interrupted: "已中断",
};

export function doneCount(plan: PlanProgress): number {
  return plan.steps.filter((s) => s.status === "done").length;
}

export function taskStatusLabel(status: string): string {
  return TASK_LABEL[status] ?? status;
}

function ProgressBar({ plan }: { plan: PlanProgress }) {
  const total = plan.steps.length;
  const done = doneCount(plan);
  const pct = total === 0 ? 0 : Math.round((done / total) * 100);
  return (
    <div
      className={`plan-bar plan-bar-${plan.status}`}
      role="progressbar"
      aria-label="计划进度"
      aria-valuemin={0}
      aria-valuemax={total}
      aria-valuenow={done}
    >
      <span className="plan-bar-fill" style={{ width: `${pct}%` }} />
    </div>
  );
}

export function PlanStepsView({
  plan,
  onResumeMessage,
  compact,
}: {
  plan: PlanProgress;
  onResumeMessage?: (msg: string) => void;
  /** 折叠时只显示一行摘要 */
  compact?: boolean;
}) {
  const refresh = usePlanStore((s) => s.refresh);
  const [busy, setBusy] = useState(false);
  const total = plan.steps.length;
  const done = doneCount(plan);

  async function onResume() {
    setBusy(true);
    try {
      const r = await taskResume(plan.id);
      onResumeMessage?.(r.message);
      if (r.outcome !== "ask_user") await refresh();
    } catch (e) {
      onResumeMessage?.(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onChoice(choice: "retry" | "skip" | "cancel") {
    setBusy(true);
    try {
      const r = await taskReentryChoice(plan.id, choice);
      onResumeMessage?.(r.message);
      await refresh();
    } catch (e) {
      onResumeMessage?.(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (compact) {
    return (
      <div className="plan-inline-summary" data-testid="plan-inline-summary">
        <span className="plan-goal">{plan.goal || "（未命名计划）"}</span>
        <span className="plan-count">
          {done}/{total}
        </span>
        <span className={`plan-task-state plan-task-${plan.status}`}>
          {taskStatusLabel(plan.status)}
        </span>
      </div>
    );
  }

  return (
    <>
      <div className="plan-head">
        <p className="plan-goal">{plan.goal || "（未命名计划）"}</p>
        <span className={`plan-task-state plan-task-${plan.status}`}>
          {taskStatusLabel(plan.status)}
        </span>
      </div>
      <ProgressBar plan={plan} />
      <p className="plan-count">
        {done}/{total} 步{plan.cancelReason ? ` · 取消原因：${plan.cancelReason}` : ""}
        {plan.interruptKind ? ` · 中断：${plan.interruptKind}` : ""}
      </p>
      {plan.resumable ? (
        <div className="plan-resume-row">
          <button
            type="button"
            className="plan-resume-btn"
            disabled={busy}
            onClick={() => void onResume()}
          >
            {busy ? "准备中…" : "准备恢复"}
          </button>
          <button
            type="button"
            className="plan-resume-btn"
            disabled={busy}
            onClick={() => void onChoice("retry")}
          >
            重试本步
          </button>
          <button
            type="button"
            className="plan-resume-btn"
            disabled={busy}
            onClick={() => void onChoice("skip")}
          >
            跳过本步
          </button>
          <button
            type="button"
            className="plan-resume-btn"
            disabled={busy}
            onClick={() => void onChoice("cancel")}
          >
            取消任务
          </button>
        </div>
      ) : null}
      <ol className="plan-steps">
        {plan.steps.map((step) => (
          <li className="plan-step" key={step.seq}>
            <span className="plan-step-seq">{step.seq + 1}</span>
            <span className="plan-step-intent">{step.intent || "（无描述）"}</span>
            {step.attempts > 1 ? (
              <span className="plan-step-attempts">×{step.attempts}</span>
            ) : null}
            <span className={`plan-chip plan-chip-${step.status}`}>
              {STEP_LABEL[step.status]}
            </span>
          </li>
        ))}
      </ol>
    </>
  );
}
