"use client";

// 日历卡（旧 calendar 段）：只读、默认关。开关拨开才触发系统授权弹窗——
// 首次启动就弹会被拒，而 macOS 的拒绝是粘性的。卡片只报计数，不显示标题。
import { useCallback, useEffect, useState } from "react";
import {
  calendarClearCache,
  calendarOpenSettings,
  calendarRequestAccess,
  calendarSetConfig,
  calendarStatus,
} from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { CalendarStatus } from "@/lib/types";

const CAL_ACCESS_TEXT: Record<string, string> = {
  full: "已授权",
  denied: "权限被拒（只能去系统设置里改）",
  restricted: "被系统限制",
  write_only: "只给了「添加日程」权限，读不了",
  not_determined: "还没授权",
};

// macOS only: deep link into the calendar privacy pane.
function openCalendarSettings() {
  if (!inTauri) return;
  calendarOpenSettings().catch(() =>
    toast("请到「系统设置 · 隐私与安全性 · 日历」里打开")
  );
}

export function CalendarCard({ active }: { active: boolean }) {
  const [st, setSt] = useState<CalendarStatus | null>(null);
  const [line, setLine] = useState("日历未开启");
  const windowShownAt = useUiStore((s) => s.windowShownAt);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setLine("浏览器预览模式：后端未连接");
      return;
    }
    try {
      setSt(await calendarStatus());
    } catch {
      setLine("日历状态读取失败");
    }
  }, []);

  // 状态渲染（旧 renderCalendar 原样）
  useEffect(() => {
    if (!st) return;
    if (!st.enabled) {
      setLine("日历未开启（默认关闭）");
      return;
    }
    if (!st.can_read) {
      setLine("看不到日程，先授权");
      return;
    }
    const bits: string[] = [];
    if (st.next) {
      const t = new Date(st.next.start_at);
      bits.push(
        `下一个 ${t.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })} ${st.next.title}`
      );
    } else {
      bits.push("接下来没安排");
    }
    bits.push(`今天读 ${st.today_reads ?? 0} 次 · 提醒 ${st.today_reminds ?? 0} 次`);
    setLine(bits.join(" · "));
  }, [st]);

  // 工具页激活或开窗断点时刷新
  useEffect(() => {
    if (active) refresh();
  }, [active, refresh, windowShownAt]);

  async function toggleEnabled(on: boolean) {
    if (!inTauri) return;
    try {
      const next = await calendarSetConfig({ enabled: on });
      setSt(next);
      // Turning it on is the ONLY moment we ask: user-initiated prompts are
      // the ones people actually accept.
      if (on && !next.can_read) {
        setSt(await calendarRequestAccess());
      }
    } catch {
      toast("日历设置失败");
    }
  }

  async function toggleRemind(on: boolean) {
    if (!inTauri) return;
    try {
      setSt(await calendarSetConfig({ remind: on }));
    } catch {
      toast("提醒设置失败");
    }
  }

  async function grant() {
    if (!inTauri) return;
    const next = await calendarRequestAccess().catch(() => null);
    if (!next) {
      openCalendarSettings();
      return;
    }
    setSt(next);
    if (!next.can_read) openCalendarSettings();
  }

  async function clearCache() {
    if (!inTauri) return;
    try {
      setSt(await calendarClearCache());
      toast("日历缓存已清空");
    } catch {
      toast("清空失败");
    }
  }

  const enabled = !!st?.enabled;
  const needGrant = enabled && !st?.can_read;

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">日历</p>
          <p className="card-sub">只读 · 日程不进记忆</p>
        </div>
        <label className="tgl tgl-master" title="总开关">
          <span className="master-label">总开关</span>
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => toggleEnabled(e.target.checked)}
          />
          <span className="track">
            <span className="knob" />
          </span>
          <span className="sr-only">开启日历</span>
        </label>
      </div>

      <div className={`subs${enabled ? "" : " off"}`}>
        <label className="row">
          <span className="row-label">会前提醒</span>
          <span className="tgl">
            <input
              type="checkbox"
              checked={!!st?.remind}
              disabled={!enabled}
              onChange={(e) => toggleRemind(e.target.checked)}
            />
            <span className="track">
              <span className="knob" />
            </span>
          </span>
        </label>
        <div className="row">
          <span className="row-label">
            {st ? CAL_ACCESS_TEXT[st.access] ?? st.access : "还没授权"}
          </span>
          {needGrant && (
            <button className="ob-btn cal-btn" onClick={grant}>
              {st?.access === "denied" || st?.access === "write_only"
                ? "打开系统设置"
                : "去授权"}
            </button>
          )}
          {!!st?.cache_count && (
            <button className="ob-btn cal-btn" onClick={clearCache}>
              清缓存
            </button>
          )}
        </div>
      </div>

      <p className="status-line">{line}</p>
    </section>
  );
}
