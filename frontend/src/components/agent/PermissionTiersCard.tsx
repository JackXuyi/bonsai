"use client";

// 权限分级卡（roadmap P4-2 / G-10）：三档说明 + 全局只读确认 + 内置工具档位一览。
// 档位权威在 Rust ToolDescriptor.access，本卡只展示。
import { useEffect, useState } from "react";
import { ApprovalModeRow } from "@/components/agent/ApprovalModeRow";
import { ToolAccessBadge } from "@/components/agent/ToolAccessBadge";
import { builtinToolsList, humanizeAgentError } from "@/lib/commands";
import {
  TOOL_ACCESS_HINT,
  TOOL_ACCESS_LABEL,
  TOOL_ACCESS_ORDER,
  normalizeToolAccess,
} from "@/lib/toolAccess";
import type { ToolDescriptor } from "@/lib/types";

export function PermissionTiersCard() {
  const [builtins, setBuiltins] = useState<ToolDescriptor[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void builtinToolsList()
      .then(setBuiltins)
      .catch((e) => {
        setBuiltins([]);
        setError(humanizeAgentError(e));
      });
  }, []);

  return (
    <div className="card agent-card" data-testid="permission-tiers-card">
      <p className="card-title">工具权限分级</p>
      <p className="hint-line">
        每个工具固定一档：只读直行、写操作每次询问、危险操作禁止。档位由引擎声明，不能在此改写。
      </p>

      <div className="agent-tier-legend" data-testid="access-tier-legend">
        {TOOL_ACCESS_ORDER.map((tier) => (
          <div key={tier} className="agent-tier-legend-row">
            <ToolAccessBadge access={tier} />
            <span className="hint-line">
              {TOOL_ACCESS_LABEL[tier]} · {TOOL_ACCESS_HINT[tier]}
            </span>
          </div>
        ))}
      </div>

      {error && <p className="model-error">{error}</p>}
      <ApprovalModeRow onError={setError} />

      <div className="agent-card-head" style={{ marginTop: 10 }}>
        <p className="card-title" style={{ marginBottom: 0, fontSize: 14 }}>
          内置工具档位
        </p>
        <span className="hint-line">{builtins.length} 个 · 随版本内置</span>
      </div>
      {builtins.length === 0 ? (
        <p className="hint-line" style={{ marginTop: 8 }}>
          没有内置工具。
        </p>
      ) : (
        <div className="agent-list" style={{ marginTop: 8 }}>
          {builtins.map((t) => {
            const access = normalizeToolAccess(t.access);
            return (
              <div key={t.name} className="agent-row" data-testid="builtin-access-row">
                <div className="agent-row-main">
                  <ToolAccessBadge access={access} />
                  <span
                    className="agent-name"
                    style={{ fontFamily: "var(--font-mono, monospace)" }}
                  >
                    {t.name}
                  </span>
                  <span className="hint-line">{t.summary}</span>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <p className="hint-line" style={{ marginTop: 8 }}>
        MCP 工具默认「只读」；写类 MCP 若未声明更高档，仍受上方「只读工具确认」约束。能力启停在「设置 · 能力」。
      </p>
    </div>
  );
}
