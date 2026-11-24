"use client";

// 设置页 · 后台循环（plan-loop-supervisor D4）：八行全列表 + D1 自动重启开关。
import { useCallback, useEffect, useState } from "react";
import {
  loopAutorestartGet,
  loopAutorestartSet,
  loopsStatus,
} from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useTauriEvent } from "@/lib/tauriListen";
import { toast } from "@/lib/stores/uiStore";
import type { LoopStatus } from "@/lib/types";

const LABELS: Record<string, string> = {
  scheduler: "夜间整理",
  predict: "预测-验证",
  growth: "形态线",
  relationship: "关系线",
  perception: "感知采样",
  face: "表情态机",
  proactive: "主动搭话",
  calendar: "日历提醒",
};

function fmtBeat(iso?: string | null) {
  if (!iso) return "—";
  const d = Date.parse(iso);
  if (!Number.isFinite(d)) return iso.slice(11, 19) || iso;
  const diff = Math.max(0, Math.floor((Date.now() - d) / 1000));
  if (diff < 5) return "刚刚";
  if (diff < 60) return `${diff}s 前`;
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  return `${Math.floor(diff / 3600)} 小时前`;
}

export function LoopsCard() {
  const [rows, setRows] = useState<LoopStatus[]>([]);
  const [auto, setAuto] = useState(false);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setRows([]);
      setLoading(false);
      return;
    }
    try {
      const [list, on] = await Promise.all([loopsStatus(), loopAutorestartGet()]);
      setRows(list);
      setAuto(on);
    } catch {
      toast("后台循环状态读不到");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), 15000);
    return () => clearInterval(t);
  }, [refresh]);

  useTauriEvent<{ name?: string }>("loop:stopped", () => {
    void refresh();
  });
  useTauriEvent<{ name?: string }>("loop:restarted", () => {
    void refresh();
  });

  async function flipAuto(next: boolean) {
    setAuto(next);
    if (!inTauri) return;
    try {
      await loopAutorestartSet(next);
      toast(
        next
          ? "已开：挂掉后每个循环本趟最多自动拉起一次"
          : "已关：循环挂掉后只留痕，不自动拉起"
      );
    } catch {
      setAuto(!next);
      toast("开关没存上");
    }
  }

  return (
    <div className="card">
      <div className="card-head">
        <div className="card-titles">
          <span className="card-title">后台循环</span>
          <span className="card-sub">八条常驻 · 挂掉可查</span>
        </div>
        <label className="tgl" title="挂掉后自动拉起一次">
          <span className="master-label">自动拉起</span>
          <input
            type="checkbox"
            checked={auto}
            onChange={(e) => void flipAuto(e.target.checked)}
          />
          <span className="track">
            <span className="knob" />
          </span>
        </label>
      </div>

      <p className="hint-line loops-hint">
        打开后：某个循环连续崩溃到上限时，本趟进程里最多自动再起一次；默认关闭。
      </p>

      {loading && rows.length === 0 ? (
        <p className="hint-line">读取中…</p>
      ) : !inTauri ? (
        <p className="hint-line">浏览器预览：后端未连接</p>
      ) : (
        <ul className="loops-list" data-testid="loops-list">
          {rows.map((r) => {
            const running = r.state === "running";
            return (
              <li key={r.name} className={running ? "on" : "off"}>
                <span
                  className={`loops-dot ${running ? "ok" : "bad"}`}
                  aria-hidden
                />
                <span className="loops-name">
                  {LABELS[r.name] ?? r.name}
                  <span className="loops-id">{r.name}</span>
                </span>
                <span className="loops-meta">
                  {running
                    ? `心跳 ${fmtBeat(r.last_beat)}`
                    : r.detail
                      ? `已停 · ${r.detail}`
                      : "已停"}
                  {r.restarts > 0 ? ` · 已拉起 ${r.restarts}` : ""}
                </span>
              </li>
            );
          })}
        </ul>
      )}

      <button type="button" className="btn btn-ghost" onClick={() => void refresh()}>
        刷新
      </button>
    </div>
  );
}
