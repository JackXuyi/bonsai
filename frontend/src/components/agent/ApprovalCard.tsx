"use client";

// 审批确认卡（spec-s4/s6）：approval_mode=ask 时，后端在执行工具前发出
// tool:approval_required，这里弹卡；用户回执走 mcp_approval_reply，
// 后端等到答复（或 30s 超时视为拒绝）才继续。
// 默认 auto（D10：默认放行），此时后端不发事件，本组件零开销。
import { useState } from "react";
import { mcpApprovalReply } from "@/lib/commands";
import { useTauriEvent } from "@/lib/tauriListen";

interface ApprovalRequest {
  id: number;
  tool: string;
  source: string;
  remote: boolean;
  argsSummary: string;
}

/**
 * 挂在 AppShell 的事件监听 + 卡片渲染。同一时刻后端只允许一个待审批
 * 请求（单线程对话），所以这里用单值状态即可。
 */
export function ApprovalCard() {
  const [req, setReq] = useState<ApprovalRequest | null>(null);
  const [stale, setStale] = useState(false);
  const [busy, setBusy] = useState(false);

  useTauriEvent<ApprovalRequest>("tool:approval_required", (payload) => {
    setStale(false);
    setBusy(false);
    setReq(payload);
  });

  if (!req) return null;

  async function reply(allow: boolean) {
    const r = req;
    if (!r) return;
    setBusy(true);
    try {
      const accepted = await mcpApprovalReply(r.id, allow);
      if (accepted) {
        setReq(null);
        setStale(false);
      } else {
        // 超时已过或已被答复：后端不再理会这张卡，告知用户别等。
        setStale(true);
      }
    } catch {
      setStale(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mconfirm" role="dialog" aria-modal="true" data-testid="approval-card">
      <div className="mconfirm-box">
        <p className="mconfirm-title">允许调用工具「{req.tool}」？</p>
        <p className="mconfirm-sub">
          来源：{req.source}
          {req.remote ? "（远程服务，参数会出设备）" : "（本机）"}
        </p>
        <pre className="agent-args">{req.argsSummary}</pre>
        {stale ? (
          <>
            <p className="mconfirm-sub">这次请求已经过期（30 秒未回应即视为拒绝）。</p>
            <div className="mconfirm-actions">
              <button
                type="button"
                className="btn-ghost"
                onClick={() => {
                  setReq(null);
                  setStale(false);
                }}
              >
                知道了
              </button>
            </div>
          </>
        ) : (
          <>
            <p className="mconfirm-sub">30 秒内不回应视为拒绝。</p>
            <div className="mconfirm-actions">
              <button
                type="button"
                className="btn-ghost"
                disabled={busy}
                onClick={() => void reply(false)}
              >
                拒绝
              </button>
              <button type="button" className="btn" disabled={busy} onClick={() => void reply(true)}>
                允许本次
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
