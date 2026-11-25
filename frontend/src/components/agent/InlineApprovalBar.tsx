"use client";

// 聊天内授权条（plan-chat-agent-inline）：替代全屏 ApprovalCard。
// 事件仍是 tool:approval_required；回执 mcp_approval_reply；等待 60s。
import { useEffect, useRef, useState } from "react";
import { mcpApprovalReply } from "@/lib/commands";
import { useTauriEvent } from "@/lib/tauriListen";

/** 与 Rust `APPROVAL_WAIT` 对齐（秒）。 */
export const APPROVAL_WAIT_SECS = 60;

interface ApprovalRequest {
  id: number;
  tool: string;
  source: string;
  remote: boolean;
  argsSummary: string;
}

export type InlineApprovalPlacement = "dock" | "toast";

export interface InlineApprovalBarProps {
  /** dock = composer 上方；toast = 非聊天页底部非模态条 */
  placement?: InlineApprovalPlacement;
}

/**
 * 单槽待批：后端同一时刻只有一个请求。Esc = 拒绝（Q1）。
 */
export function InlineApprovalBar({ placement = "dock" }: InlineApprovalBarProps) {
  const [req, setReq] = useState<ApprovalRequest | null>(null);
  const [stale, setStale] = useState(false);
  const [busy, setBusy] = useState(false);
  const [leftSecs, setLeftSecs] = useState(APPROVAL_WAIT_SECS);
  const allowRef = useRef<HTMLButtonElement>(null);
  const startedAt = useRef<number>(0);

  useTauriEvent<ApprovalRequest>("tool:approval_required", (payload) => {
    setStale(false);
    setBusy(false);
    setReq(payload);
    startedAt.current = Date.now();
    setLeftSecs(APPROVAL_WAIT_SECS);
  });

  useEffect(() => {
    if (!req || stale) return;
    const tick = () => {
      const left = Math.max(
        0,
        APPROVAL_WAIT_SECS - Math.floor((Date.now() - startedAt.current) / 1000),
      );
      setLeftSecs(left);
      if (left === 0) setStale(true);
    };
    tick();
    const id = window.setInterval(tick, 250);
    return () => window.clearInterval(id);
  }, [req, stale]);

  useEffect(() => {
    if (!req || stale) return;
    allowRef.current?.focus();
  }, [req, stale]);

  useEffect(() => {
    if (!req || stale) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        void reply(false);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // reply closes over req; rebind when req changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [req, stale, busy]);

  if (!req) return null;

  async function reply(allow: boolean) {
    const r = req;
    if (!r || busy) return;
    setBusy(true);
    try {
      const accepted = await mcpApprovalReply(r.id, allow);
      if (accepted) {
        setReq(null);
        setStale(false);
      } else {
        setStale(true);
      }
    } catch {
      setStale(true);
    } finally {
      setBusy(false);
    }
  }

  const rootClass =
    placement === "toast" ? "inline-approval inline-approval-toast" : "inline-approval";

  return (
    <div
      className={rootClass}
      role="region"
      aria-label="工具调用授权"
      data-testid="approval-card"
      data-placement={placement}
    >
      <p className="inline-approval-title">允许调用工具「{req.tool}」？</p>
      <p className="inline-approval-sub">
        来源：{req.source}
        {req.remote ? "（远程服务，参数会出设备）" : "（本机）"}
      </p>
      <pre className="agent-args inline-approval-args">{req.argsSummary}</pre>
      {stale ? (
        <>
          <p className="inline-approval-sub">
            这次请求已经过期（1 分钟未回应即视为拒绝）。
          </p>
          <div className="inline-approval-actions">
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
          <p className="inline-approval-sub" data-testid="approval-countdown">
            {leftSecs} 秒内不回应视为拒绝。
          </p>
          <div className="inline-approval-actions">
            <button
              type="button"
              className="btn-ghost"
              disabled={busy}
              onClick={() => void reply(false)}
            >
              拒绝
            </button>
            <button
              type="button"
              className="btn"
              ref={allowRef}
              disabled={busy}
              onClick={() => void reply(true)}
            >
              允许本次
            </button>
          </div>
        </>
      )}
    </div>
  );
}
