"use client";

// 完整审计时间线（roadmap P4-2 / G-11）：参数摘要 + 结果状态 + 时间戳可筛选。
// 数据源复用 tool_call_records（limit ≤200）；前端筛选，不平行造查询。
import { useCallback, useEffect, useMemo, useState } from "react";
import { toolCallRecords } from "@/lib/commands";
import type { ToolCallRecord } from "@/lib/types";

const FETCH_LIMIT = 200;

type OutcomeFilter = "all" | "ok" | "denied" | "error";
type KindFilter = "all" | "mcp" | "skill" | "builtin" | "other";

function outcomeClass(outcome: string): string {
  if (outcome === "ok") return "agent-dot ok";
  if (outcome === "denied") return "agent-dot denied";
  return "agent-dot error";
}

function kindOf(tool: string): KindFilter {
  if (tool === "mcp" || tool === "skill" || tool === "builtin") return tool;
  return "other";
}

function formatTs(ts: string): string {
  // 兼容 ISO：显示月-日 时:分（与旧卡一致）
  if (ts.length >= 16) return ts.slice(5, 16).replace("T", " ");
  return ts;
}

export interface AuditTimelineProps {
  /** compact = 运维速览（20 条、无筛选）；full = 权限分区完整页。 */
  variant?: "compact" | "full";
}

export function AuditTimeline({ variant = "full" }: AuditTimelineProps) {
  const full = variant === "full";
  const [rows, setRows] = useState<ToolCallRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [outcome, setOutcome] = useState<OutcomeFilter>("all");
  const [kind, setKind] = useState<KindFilter>("all");
  const [remoteOnly, setRemoteOnly] = useState(false);
  const [query, setQuery] = useState("");

  const refresh = useCallback(async () => {
    try {
      setRows(await toolCallRecords(full ? FETCH_LIMIT : 20));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [full]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    if (!full) return rows;
    const q = query.trim().toLowerCase();
    return rows.filter((r) => {
      if (outcome !== "all" && r.outcome !== outcome) return false;
      if (kind !== "all" && kindOf(r.tool) !== kind) return false;
      if (remoteOnly && !r.remote) return false;
      if (q) {
        const hay = `${r.tool}:${r.action} ${r.paramsSummary ?? ""} ${r.resultStatus ?? ""}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [rows, full, outcome, kind, remoteOnly, query]);

  return (
    <div
      className="card agent-card"
      data-testid={full ? "audit-timeline" : "tool-records"}
    >
      <div className="agent-card-head">
        <div>
          <p className="card-title">{full ? "工具调用审计" : "调用记录"}</p>
          {full && (
            <p className="hint-line" style={{ margin: 0 }}>
              全部工具调用时间线（最近 {FETCH_LIMIT} 条）· 参数已脱敏
            </p>
          )}
        </div>
        <button type="button" className="btn-ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>

      {full && (
        <div className="agent-audit-filters" data-testid="audit-filters">
          <label className="agent-audit-filter">
            <span className="hint-line">结果</span>
            <select
              value={outcome}
              onChange={(e) => setOutcome(e.target.value as OutcomeFilter)}
              aria-label="按结果筛选"
            >
              <option value="all">全部</option>
              <option value="ok">成功</option>
              <option value="denied">拒绝</option>
              <option value="error">错误</option>
            </select>
          </label>
          <label className="agent-audit-filter">
            <span className="hint-line">来源</span>
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as KindFilter)}
              aria-label="按来源筛选"
            >
              <option value="all">全部</option>
              <option value="builtin">内置</option>
              <option value="mcp">MCP</option>
              <option value="skill">技能</option>
              <option value="other">其他</option>
            </select>
          </label>
          <label className="agent-audit-filter agent-audit-check">
            <input
              type="checkbox"
              checked={remoteOnly}
              onChange={(e) => setRemoteOnly(e.target.checked)}
            />
            <span className="hint-line">仅远程</span>
          </label>
          <label className="agent-audit-filter agent-audit-search">
            <span className="hint-line">搜索</span>
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="工具名 / 参数 / 状态"
              aria-label="搜索审计记录"
            />
          </label>
        </div>
      )}

      {error && <p className="model-error">{error}</p>}
      {filtered.length === 0 ? (
        <p className="hint-line">
          {rows.length === 0
            ? "还没有工具调用。MCP / 内置 / 技能每次被使用都会在这里留痕。"
            : "没有符合筛选条件的记录。"}
        </p>
      ) : (
        <>
          {full && (
            <p className="hint-line" data-testid="audit-count">
              显示 {filtered.length} / {rows.length} 条
            </p>
          )}
          <div className="agent-list">
            {filtered.map((r) => (
              <div key={r.id}>
                <button
                  type="button"
                  className="agent-record"
                  onClick={() => setOpen(open === r.id ? null : r.id)}
                >
                  <span className={outcomeClass(r.outcome)} aria-hidden />
                  <span className="agent-record-ts">{formatTs(r.ts)}</span>
                  <span
                    className="agent-transport"
                    data-transport={r.tool === "builtin" ? "builtin" : r.tool === "http" ? "http" : undefined}
                  >
                    {r.tool}
                  </span>
                  <span className="agent-record-tool">{r.action}</span>
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
        </>
      )}
    </div>
  );
}
