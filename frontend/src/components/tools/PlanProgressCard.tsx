"use client";

// 计划进度卡（docs/zh/plan-planning.md §5.6，T6 / A13–A15）：工具页只读视图。
// 唯一状态源是 planStore（回放 plan_list + 实时 task_* 事件）——本组件不自己
// 监听 Channel、不轮询、不可勾选；过程也不进对话气泡（11-22 裁决）。
// Phase 3 A1：可恢复任务显式「准备恢复」按钮（D1 不自动续跑）。
import { useEffect, useState } from "react";
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

function doneCount(plan: PlanProgress): number {
  return plan.steps.filter((s) => s.status === "done").length;
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

function PlanDetail({
  plan,
  onResumeMessage,
}: {
  plan: PlanProgress;
  onResumeMessage: (msg: string) => void;
}) {
  const refresh = usePlanStore((s) => s.refresh);
  const [busy, setBusy] = useState(false);
  const total = plan.steps.length;
  const done = doneCount(plan);

  async function onResume() {
    setBusy(true);
    try {
      const r = await taskResume(plan.id);
      onResumeMessage(r.message);
      if (r.outcome === "ask_user") {
        // 留给下方三个按钮；不自动选
      } else {
        await refresh();
      }
    } catch (e) {
      onResumeMessage(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onChoice(choice: "retry" | "skip" | "cancel") {
    setBusy(true);
    try {
      const r = await taskReentryChoice(plan.id, choice);
      onResumeMessage(r.message);
      await refresh();
    } catch (e) {
      onResumeMessage(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="plan-head">
        <p className="plan-goal">{plan.goal || "（未命名计划）"}</p>
        <span className={`plan-task-state plan-task-${plan.status}`}>
          {TASK_LABEL[plan.status] ?? plan.status}
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

export function PlanProgressCard({ active }: { active: boolean }) {
  const current = usePlanStore((s) => s.current);
  const history = usePlanStore((s) => s.history);
  const refresh = usePlanStore((s) => s.refresh);
  const [showHistory, setShowHistory] = useState(false);
  const [resumeMsg, setResumeMsg] = useState<string | null>(null);

  // 首屏 / 切页回来拉一次基线；执行中由 task_* 事件推进（不轮询）。
  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);

  const older = history.filter((p) => p.id !== current?.id);

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">计划进度</p>
          <p className="card-sub">多步的事先列计划 · 只读 · 过程不刷屏对话</p>
        </div>
      </div>
      {current ? (
        <PlanDetail plan={current} onResumeMessage={setResumeMsg} />
      ) : (
        <p className="status-line">还没有计划——多步的事（先查再写、先看再改）它会先列个计划</p>
      )}
      {resumeMsg ? <p className="status-line" role="status">{resumeMsg}</p> : null}
      {older.length > 0 ? (
        <>
          <button
            className="plan-history-toggle"
            aria-expanded={showHistory}
            onClick={() => setShowHistory((v) => !v)}
          >
            {showHistory ? "收起历史" : `查看历史（${older.length}）`}
          </button>
          {showHistory ? (
            <ul className="plan-history">
              {older.map((p) => (
                <li className="plan-history-row" key={p.id}>
                  <span className="plan-history-goal">{p.goal || "（未命名计划）"}</span>
                  <span className="plan-history-count">
                    {doneCount(p)}/{p.steps.length}
                  </span>
                  <span className={`plan-task-state plan-task-${p.status}`}>
                    {TASK_LABEL[p.status] ?? p.status}
                  </span>
                </li>
              ))}
            </ul>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
