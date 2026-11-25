"use client";

// 云端循环预算开关（plan-chat-agent-inline）：默认关；开启后上限固定 32。
import { useCallback, useEffect, useState } from "react";
import { agentBudgetGet, agentBudgetSet, humanizeAgentError } from "@/lib/commands";

export interface BudgetCapsRowProps {
  onError?: (msg: string | null) => void;
}

export function BudgetCapsRow({ onError }: BudgetCapsRowProps) {
  const [toolOn, setToolOn] = useState(false);
  const [requestOn, setRequestOn] = useState(false);
  const [toolCap, setToolCap] = useState(32);
  const [requestCap, setRequestCap] = useState(32);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    void agentBudgetGet()
      .then((r) => {
        setToolOn(r.toolCapEnabled);
        setRequestOn(r.requestCapEnabled);
        setToolCap(r.toolCap);
        setRequestCap(r.requestCap);
        setLoaded(true);
      })
      .catch(() => {
        setLoaded(true);
      });
  }, []);

  const persist = useCallback(
    async (nextTool: boolean, nextReq: boolean) => {
      setToolOn(nextTool);
      setRequestOn(nextReq);
      try {
        await agentBudgetSet(nextTool, nextReq);
        onError?.(null);
      } catch (e) {
        setToolOn(!nextTool);
        setRequestOn(!nextReq);
        onError?.(humanizeAgentError(e));
      }
    },
    [onError],
  );

  return (
    <div className="agent-approval agent-budget" data-testid="budget-caps-row">
      <span className="hint-line">云端循环上限</span>
      <button
        type="button"
        className={toolOn ? "btn" : "btn-ghost"}
        disabled={!loaded}
        data-testid="budget-tool-toggle"
        onClick={() => void persist(!toolOn, requestOn)}
      >
        工具 {toolCap} 次
      </button>
      <button
        type="button"
        className={requestOn ? "btn" : "btn-ghost"}
        disabled={!loaded}
        data-testid="budget-request-toggle"
        onClick={() => void persist(toolOn, !requestOn)}
      >
        HTTP {requestCap} 次
      </button>
      <p className="hint-line agent-approval-note" data-testid="budget-caps-hint">
        {toolOn || requestOn
          ? `已开启的项按 ${Math.max(toolCap, requestCap)} 强制收尾；未开启的项仍有安全上限 ${toolCap}。`
          : `默认不主动限制；仍有安全上限 ${toolCap}（防失控）。`}
      </p>
    </div>
  );
}
