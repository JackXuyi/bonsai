"use client";

// 设置页 · 诊断（plan-retry §7.4 + plan-store-lock T4）：最近失败 + 写锁采样。
import { useCallback, useEffect, useState } from "react";
import { failuresRecent, storeLockStats } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "@/lib/stores/uiStore";
import type { FailureRecent, StoreLockStats } from "@/lib/types";

const SOURCE_LABEL: Record<string, string> = {
  turn: "对话",
  consolidate: "夜间整理",
};

function fmtAt(iso: string) {
  const d = Date.parse(iso);
  if (!Number.isFinite(d)) return iso;
  const dt = new Date(d);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(dt.getMonth() + 1)}-${pad(dt.getDate())} ${pad(dt.getHours())}:${pad(dt.getMinutes())}`;
}

export function DiagnosticsCard() {
  const [rows, setRows] = useState<FailureRecent[]>([]);
  const [locks, setLocks] = useState<StoreLockStats | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setRows([]);
      setLocks(null);
      setLoading(false);
      return;
    }
    try {
      const [list, stats] = await Promise.all([
        failuresRecent(10),
        storeLockStats().catch(() => null),
      ]);
      setRows(list);
      setLocks(stats);
    } catch {
      toast("诊断列表读不到");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="card">
      <div className="card-head">
        <div className="card-titles">
          <span className="card-title">诊断</span>
          <span className="card-sub">最近失败 · 写锁采样 · 只读</span>
        </div>
      </div>

      <p className="hint-line diagnostics-hint">
        对话失败轮与夜间整理失败会落在这里；不弹 toast，也不在此重跑。
      </p>

      {locks ? (
        <p className="hint-line" data-testid="diagnostics-lock-stats">
          写锁 · 获取 {locks.write_acquires} · 均等待 {locks.write_wait_ms_avg}ms ·
          慢等待(&gt;50ms) {locks.write_wait_slow} · 均持锁 {locks.write_hold_ms_avg}ms
        </p>
      ) : null}

      {loading && rows.length === 0 ? (
        <p className="hint-line">读取中…</p>
      ) : !inTauri ? (
        <p className="hint-line">浏览器预览：后端未连接</p>
      ) : rows.length === 0 ? (
        <p className="hint-line" data-testid="diagnostics-empty">
          最近没有记到失败
        </p>
      ) : (
        <ul className="diagnostics-list" data-testid="diagnostics-list">
          {rows.map((r, i) => (
            <li key={`${r.source}-${r.at}-${i}`}>
              <span className="diagnostics-source">
                {SOURCE_LABEL[r.source] ?? r.source}
              </span>
              <span className="diagnostics-body">
                <span className="diagnostics-hint-text">{r.hint}</span>
                <span className="diagnostics-meta">
                  {r.kind} · {fmtAt(r.at)}
                </span>
              </span>
            </li>
          ))}
        </ul>
      )}

      <button type="button" className="btn btn-ghost" onClick={() => void refresh()}>
        刷新
      </button>
    </div>
  );
}
