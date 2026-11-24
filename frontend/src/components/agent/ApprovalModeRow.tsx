"use client";

// 全局审批模式：auto | ask（spec-s4/s6）。只影响 read 档；ask/forbidden 档由引擎硬约束。
import { useCallback, useEffect, useState } from "react";
import { agentApprovalGet, agentApprovalSet, humanizeAgentError } from "@/lib/commands";

export interface ApprovalModeRowProps {
  /** 切换失败时回调（父卡展示错误）。 */
  onError?: (msg: string | null) => void;
}

export function ApprovalModeRow({ onError }: ApprovalModeRowProps) {
  const [approval, setApproval] = useState<"auto" | "ask">("auto");

  useEffect(() => {
    void agentApprovalGet()
      .then(setApproval)
      .catch(() => setApproval("auto"));
  }, []);

  const switchApproval = useCallback(
    async (next: "auto" | "ask") => {
      setApproval(next);
      try {
        await agentApprovalSet(next);
        onError?.(null);
      } catch (e) {
        setApproval(next === "ask" ? "auto" : "ask");
        onError?.(humanizeAgentError(e));
      }
    },
    [onError],
  );

  return (
    <div className="agent-approval" data-testid="approval-mode-row">
      <span className="hint-line">只读工具确认</span>
      <button
        type="button"
        className={approval === "auto" ? "btn" : "btn-ghost"}
        onClick={() => void switchApproval("auto")}
      >
        自动放行
      </button>
      <button
        type="button"
        className={approval === "ask" ? "btn" : "btn-ghost"}
        onClick={() => void switchApproval("ask")}
      >
        每次询问
      </button>
      <p className="hint-line agent-approval-note" data-testid="approval-mode-hint">
        {approval === "ask"
          ? "只读工具也会先弹确认卡；写类工具本来就会询问。30 秒不回应视为拒绝。"
          : "只读工具直接执行；写类仍强制询问，禁止类永不执行。"}
      </p>
    </div>
  );
}
