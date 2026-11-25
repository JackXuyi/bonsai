"use client";

// 聊天内计划进度（plan-chat-agent-inline M3）：气泡只存 planTaskId，
// 状态权威在 planStore。
import { useEffect, useState } from "react";
import { usePlanStore } from "@/lib/stores/planStore";
import { PlanStepsView } from "@/components/tools/PlanStepsView";

export function PlanInlineBubble({ planTaskId }: { planTaskId: string }) {
  const plan = usePlanStore((s) => {
    if (s.current?.id === planTaskId) return s.current;
    return s.history.find((p) => p.id === planTaskId) ?? null;
  });
  const [collapsed, setCollapsed] = useState(false);
  const [resumeMsg, setResumeMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!plan) return;
    if (plan.status === "done" || plan.status === "canceled" || plan.status === "failed") {
      setCollapsed(true);
    } else if (plan.status === "running" || plan.status === "interrupted") {
      setCollapsed(false);
    }
  }, [plan?.status, plan]);

  if (!plan) {
    return (
      <div className="plan-inline" data-testid="plan-inline">
        <p className="hint-line">计划加载中…</p>
      </div>
    );
  }

  return (
    <div className="plan-inline" data-testid="plan-inline">
      <button
        type="button"
        className="plan-inline-toggle"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed((v) => !v)}
      >
        {collapsed ? "展开计划" : "收起计划"}
      </button>
      <PlanStepsView
        plan={plan}
        compact={collapsed}
        onResumeMessage={setResumeMsg}
      />
      {resumeMsg ? (
        <p className="status-line" role="status">
          {resumeMsg}
        </p>
      ) : null}
    </div>
  );
}
