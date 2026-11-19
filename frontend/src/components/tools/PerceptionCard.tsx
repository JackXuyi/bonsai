"use client";

// 感知层卡（旧 perception 段）：总开关 + 三个子开关 + 状态行。
// 总开关是隐私开关，拨动时明说发生了什么。
import { useCallback, useEffect, useState } from "react";
import { perceptionGetConfig, perceptionSetConfig, perceptionStatus } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { PerceptionConfig } from "@/lib/types";

function fmtIdle(secs: number) {
  if (!Number.isFinite(secs) || secs > 24 * 3600) return "未知";
  if (secs < 60) return "刚刚动过";
  const min = Math.floor(secs / 60);
  if (min < 60) return `空闲 ${min} 分钟`;
  return `空闲 ${Math.floor(min / 60)} 小时 ${min % 60} 分`;
}

export function PerceptionCard({ active }: { active: boolean }) {
  const setToolResult = useUiStore((s) => s.setToolResult);
  const [cfg, setCfg] = useState<PerceptionConfig | null>(null);
  const [status, setStatus] = useState<{ enabled: boolean; idle_secs?: number; front_app?: string } | null>(null);
  const [line, setLine] = useState("感知层未开启");
  const [dotOn, setDotOn] = useState(false);

  const refreshStatus = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      setDotOn(false);
      return;
    }
    try {
      const s = await perceptionStatus();
      setStatus(s);
      if (!s.enabled) {
        setLine("感知层未开启（默认关闭）");
        setDotOn(false);
        return;
      }
      const parts = [fmtIdle(s.idle_secs ?? NaN)];
      if (s.front_app) parts.push(`前台 ${s.front_app}`);
      setLine(parts.join(" · "));
      setDotOn(true);
    } catch {
      setLine("感知状态读取失败");
      setDotOn(false);
    }
  }, []);

  const loadConfig = useCallback(async () => {
    if (!inTauri) return;
    try {
      setCfg(await perceptionGetConfig());
    } catch {
      /* perception state not ready yet — the poll below will retry */
    }
  }, []);

  // 旧 selectTab("tools") 时刷新 + 15s 轮询
  useEffect(() => {
    if (active) refreshStatus();
  }, [active, refreshStatus]);

  useEffect(() => {
    loadConfig();
    refreshStatus();
    const t = setInterval(refreshStatus, 15000);
    return () => clearInterval(t);
  }, [loadConfig, refreshStatus]);

  async function save(next: PerceptionConfig, masterFlip?: boolean) {
    setCfg(next);
    if (masterFlip !== undefined) {
      toast(
        masterFlip ? "感知层已开启 · 原文不出机器" : "感知层已关闭"
      );
    }
    if (!inTauri) return;
    try {
      await perceptionSetConfig(next);
    } catch (err) {
      setToolResult({ title: "感知设置保存失败", text: String(err) });
      toast("感知设置没存上");
    }
    refreshStatus();
  }

  const enabled = !!cfg?.enabled;
  const off = !enabled;

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">感知层</p>
          <p className="card-sub">默认关闭 · 原文不出机器</p>
        </div>
        <label className="tgl tgl-master" title="总开关">
          <span className="master-label">总开关</span>
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) =>
              save(
                {
                  enabled: e.target.checked,
                  clipboard: !!cfg?.clipboard,
                  active_app: !!cfg?.active_app,
                  downloads: !!cfg?.downloads,
                },
                e.target.checked
              )
            }
          />
          <span className="track">
            <span className="knob" />
          </span>
          <span className="sr-only">开启感知层</span>
        </label>
      </div>

      <div className={`subs${off ? " off" : ""}`}>
        <label className="row">
          <span className="row-ico">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <rect x="8" y="2" width="8" height="4" rx="1" />
              <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
            </svg>
          </span>
          <span className="row-label">剪贴板</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.clipboard}
              disabled={off}
              onChange={(e) =>
                cfg &&
                save({ ...cfg, clipboard: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        <label className="row">
          <span className="row-ico">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <rect x="2" y="4" width="20" height="16" rx="2" />
              <path d="M2 9h20" />
              <path d="M6 4v3" />
            </svg>
          </span>
          <span className="row-label">前台应用</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.active_app}
              disabled={off}
              onChange={(e) =>
                cfg &&
                save({ ...cfg, active_app: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>

        <label className="row">
          <span className="row-ico">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
          </span>
          <span className="row-label">下载目录</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!cfg?.downloads}
              disabled={off}
              onChange={(e) =>
                cfg &&
                save({ ...cfg, downloads: e.target.checked })
              }
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>
      </div>

      <p className="status-line">
        <span className={`dot${dotOn ? " on" : ""}`} />
        <span>{line}</span>
      </p>
    </section>
  );
}
