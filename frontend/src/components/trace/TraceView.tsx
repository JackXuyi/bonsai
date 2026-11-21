"use client";

// 链路追踪视图（spec-s8 S8-d）：数据获取 + 回合摘要卡 + 时间线。
// /trace/[turnId] 直取；/trace?event=… 先反查（历史气泡的懒查入口）。
import { useEffect, useState } from "react";
import { turnTraceFindByEvent, turnTraceGet } from "@/lib/commands";
import { formatWhen } from "@/lib/format";
import type { TurnTrace } from "@/lib/types";
import { TraceTimeline } from "./TraceTimeline";

const KIND_LABEL: Record<TurnTrace["kind"], string> = {
  model: "模型回合",
  tool: "工具回合",
  crisis: "危机拦截",
  proactive: "主动搭话",
};

type LoadState = "loading" | "missing" | "error" | "ready";

export function TraceView({ turnId, eventId }: { turnId?: number; eventId?: number }) {
  const [trace, setTrace] = useState<TurnTrace | null>(null);
  const [state, setState] = useState<LoadState>("loading");

  useEffect(() => {
    let cancelled = false;
    setState("loading");
    void (async () => {
      try {
        let id = turnId;
        if (!id && eventId) id = (await turnTraceFindByEvent(eventId)) ?? undefined;
        const t = id ? await turnTraceGet(id) : null;
        if (cancelled) return;
        if (t) {
          setTrace(t);
          setState("ready");
        } else {
          setState("missing");
        }
      } catch {
        if (!cancelled) setState("error");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [turnId, eventId]);

  if (state === "loading") {
    return <p className="hint-line trace-empty">链路读取中…</p>;
  }
  if (state === "missing" || state === "error") {
    return (
      <div className="trace-empty">
        <p className="empty-title">{state === "error" ? "链路读取失败" : "没有留下记录"}</p>
        <p className="empty-sub">
          {state === "error"
            ? "本地数据库读取出错了，稍后再试。"
            : "这条消息产生于链路追踪上线之前，没有留下记录。"}
        </p>
      </div>
    );
  }
  if (!trace) return null;

  return (
    <div className="trace-view">
      <div className="trace-summary card">
        <div className="trace-badges">
          <span className="trace-badge kind">{KIND_LABEL[trace.kind]}</span>
          {trace.toolLabel && <span className="trace-badge">{trace.toolLabel}</span>}
          {trace.durationMs != null && (
            <span className="trace-badge">总耗时 {Math.max(1, Math.round(trace.durationMs))}ms</span>
          )}
          {trace.filterReport && <FilterBadge report={trace.filterReport} />}
        </div>
        {trace.userText && <p className="trace-line user-line">用户：{trace.userText}</p>}
        {trace.replyText && <p className="trace-line">回复：{trace.replyText}</p>}
        <p className="trace-hint-line">
          {formatWhen(trace.ts)} · {trace.steps.length} 步 · 会话 {trace.sessionId}
        </p>
      </div>
      <TraceTimeline steps={trace.steps} />
    </div>
  );
}

/** 过滤报告徽标：有顶嘴证据标黄，全过标绿。 */
function FilterBadge({ report }: { report: string }) {
  let explicit = false;
  let hedged = false;
  try {
    const parsed = JSON.parse(report) as { explicit?: boolean; hedged?: boolean };
    explicit = Boolean(parsed.explicit);
    hedged = Boolean(parsed.hedged);
  } catch {
    // 坏 JSON 就当没看见——摘要卡不是排障工具本身
  }
  if (explicit || hedged) {
    return <span className="trace-badge warn">过滤：有顶嘴证据</span>;
  }
  return <span className="trace-badge ok">过滤通过</span>;
}
