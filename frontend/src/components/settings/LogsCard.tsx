"use client";

// 设置页 · 日志卡（spec-m4 §4B）：应用日志 / sidecar 日志双源，
// 尾部 200 行等宽深色渲染 + 打开目录。只读——日志不是给人编辑的。
import { useState } from "react";
import { debugOpenLogDir } from "@/lib/commands";
import { classifyLine } from "@/lib/logLines";
import { useLogsPolling } from "@/hooks/useLogsPolling";
import type { LogFile } from "@/lib/types";

const TABS: { id: LogFile; label: string }[] = [
  { id: "app_log", label: "应用日志" },
  { id: "sidecar_log", label: "sidecar 日志" },
];

export function LogsCard() {
  const [file, setFile] = useState<LogFile>("app_log");
  const { rows, loading, refresh } = useLogsPolling(file);

  return (
    <div className="card">
      <div className="card-head">
        <span className="card-title">日志</span>
        <span className="card-sub">本地落盘 · 尾部 200 行</span>
      </div>

      <div className="log-tabs" role="tablist" aria-label="日志来源">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={file === t.id}
            className={file === t.id ? "on" : ""}
            onClick={() => setFile(t.id)}
          >
            {t.label}
          </button>
        ))}
        <span className="log-tabs-spacer" />
        <button
          type="button"
          className="log-dir-btn"
          onClick={() => void debugOpenLogDir()}
        >
          打开目录
        </button>
      </div>

      <div className="logbox" data-testid="logbox">
        {rows.length === 0 ? (
          <div className="log-empty">{loading ? "读取中…" : "暂无日志"}</div>
        ) : (
          rows.map((line, i) => (
            <div key={i} className={`lv-${classifyLine(line)}`}>
              {line}
            </div>
          ))
        )}
      </div>

      <button type="button" className="btn btn-ghost" onClick={() => void refresh()}>
        刷新
      </button>
    </div>
  );
}
