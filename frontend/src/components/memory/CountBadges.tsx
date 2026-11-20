"use client";

// 计数徽章行（spec-m3 §2）：一眼看清「它记住了多少」。
// 预测徽章是命中率，不是总数——分母为 0 时给一句人话，不显示 NaN。
import type { MemoryCounts } from "@/lib/types";

export function CountBadges({ counts }: { counts: MemoryCounts }) {
  const accuracy =
    counts.predictionsTotal === 0
      ? "还没有预测"
      : `${counts.predictionsHit}/${counts.predictionsTotal}`;

  return (
    <div className="mem-badges">
      <span className="mem-badge">
        事件 <b>{counts.events}</b>
      </span>
      <span className="mem-badge">
        事实 <b>{counts.facts}</b>
      </span>
      <span className="mem-badge">
        偏好 <b>{counts.preferences}</b>
      </span>
      <span className="mem-badge">
        边界 <b>{counts.boundaries}</b>
      </span>
      <span className="mem-badge" title="猜中次数 / 问过次数">
        预测命中 <b>{accuracy}</b>
      </span>
      <span className="mem-badge">
        工具调用 <b>{counts.toolCalls}</b>
      </span>
      <span className="mem-badge" title="日程缓存是纯缓存，随时可清">
        日历缓存 <b>{counts.calendarCache}</b>
      </span>
    </div>
  );
}
