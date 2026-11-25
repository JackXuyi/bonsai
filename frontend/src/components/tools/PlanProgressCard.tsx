"use client";

// 计划进度卡（管理面）：完整历史在此；进行中也可在对话里看
// （plan-chat-agent-inline 推翻 D6「不进气泡」）。
import { useEffect, useState } from "react";
import { usePlanStore } from "@/lib/stores/planStore";
import { doneCount, PlanStepsView, taskStatusLabel } from "./PlanStepsView";

export function PlanProgressCard({ active }: { active: boolean }) {
  const current = usePlanStore((s) => s.current);
  const history = usePlanStore((s) => s.history);
  const refresh = usePlanStore((s) => s.refresh);
  const [showHistory, setShowHistory] = useState(false);
  const [resumeMsg, setResumeMsg] = useState<string | null>(null);

  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);

  const older = history.filter((p) => p.id !== current?.id);

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">计划进度</p>
          <p className="card-sub">完整历史在此；进行中也可在对话里看</p>
        </div>
      </div>
      {current ? (
        <PlanStepsView plan={current} onResumeMessage={setResumeMsg} />
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
                    {taskStatusLabel(p.status)}
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
