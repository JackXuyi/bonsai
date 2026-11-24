"use client";

// P4-3 / G-18：导出当日对话·工具·安全时间线（本地 Markdown，不出机）。
import { useCallback, useState } from "react";
import { observabilityDayExport } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "@/lib/stores/uiStore";
import type { DayTimelineExport } from "@/lib/types";

export function ObservabilityCard() {
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<DayTimelineExport | null>(null);

  const exportToday = useCallback(async () => {
    if (!inTauri) {
      toast("浏览器预览模式：后端未连接");
      return;
    }
    setBusy(true);
    try {
      const exp = await observabilityDayExport();
      setLast(exp);
      try {
        await navigator.clipboard.writeText(exp.markdown);
        toast(
          `已复制今日时间线（回合 ${exp.turns} · 工具 ${exp.tools} · 安全 ${exp.security}）`
        );
      } catch {
        toast("导出成功，但剪贴板写入失败——见下方预览");
      }
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  return (
    <section className="card" data-testid="observability-card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">可观测</p>
          <p className="card-sub">导出今日决策 / 工具 / 安全时间线</p>
        </div>
        <button
          type="button"
          className="ob-btn cal-btn"
          onClick={exportToday}
          disabled={busy}
        >
          {busy ? "导出中…" : "复制今日 Markdown"}
        </button>
      </div>
      <p className="hint-line">
        只读本地库，默认不出机。含注入降级留痕（security）便于自查。
      </p>
      {last && (
        <pre className="obs-preview" data-testid="obs-preview">
          {last.markdown}
        </pre>
      )}
    </section>
  );
}
