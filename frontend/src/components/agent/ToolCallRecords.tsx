"use client";

// 调用记录卡（spec-s6 §3）：最近 20 条审计只读总览，透明可审查（D8）。
import { useCallback, useEffect, useState } from "react";
import { toolCallRecords } from "@/lib/commands";
import type { ToolCallRecord } from "@/lib/types";

function outcomeClass(outcome: string): string {
  if (outcome === "ok") return "agent-dot ok";
  if (outcome === "denied") return "agent-dot denied";
  return "agent-dot error";
}

export function ToolCallRecords() {
  const [rows, setRows] = useState<ToolCallRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setRows(await toolCallRecords(20));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="card agent-card" data-testid="tool-records">
      <div className="agent-card-head">
        <p className="card-title">调用记录</p>
        <button type="button" className="btn-ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>
      {error && <p className="model-error">{error}</p>}
      {rows.length === 0 ? (
        <p className="hint-line">还没有工具调用。MCP 工具 / 技能包每次被使用都会在这里留痕。</p>
      ) : (
        <div className="agent-list">
          {rows.map((r) => (
            <div key={r.id}>
              <button
                type="button"
                className="agent-record"
                onClick={() => setOpen(open === r.id ? null : r.id)}
              >
                <span className={outcomeClass(r.outcome)} aria-hidden />
                <span className="agent-record-ts">{r.ts.slice(5, 16).replace("T", " ")}</span>
                <span className="agent-record-tool">
                  {r.tool}:{r.action}
                </span>
                {r.remote && <span className="agent-remote">remote</span>}
                <span className="hint-line">
                  {r.durationMs != null ? `${r.durationMs}ms` : r.outcome}
                </span>
              </button>
              {open === r.id && (
                <div className="agent-record-detail">
                  {r.paramsSummary && <p>参数：{r.paramsSummary}</p>}
                  <p>状态：{r.resultStatus ?? r.outcome}</p>
                  {r.promptTrace && <p className="hint-line">trace：{r.promptTrace}</p>}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
