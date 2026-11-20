"use client";

// 记忆页组装（spec-m3 §2）：徽章行 + 事件 + 事实/偏好/边界 + 最近预测 + 动作。
// 原则：只展示，不提供编辑——记忆可看是信任，可改是作弊。
import { formatWhen } from "@/lib/format";
import { useMemoryOverview } from "@/hooks/useMemoryOverview";
import { CountBadges } from "./CountBadges";
import { EventList } from "./EventList";
import { FactsPrefsList } from "./FactsPrefsList";
import { MemoryActions } from "./MemoryActions";
import { PredictionCard } from "./PredictionCard";

export function MemoryOverview() {
  const { data, boundaries, loading, error, refresh } = useMemoryOverview();

  if (error) {
    return (
      <>
        <p className="model-error">读取记忆失败：{error}</p>
        <button type="button" className="btn" onClick={() => void refresh()}>
          重试
        </button>
      </>
    );
  }

  // 首次加载才显示空态；刷新时保留旧数据，避免闪一下空白。
  if (!data) {
    return <p className="hint-line">{loading ? "读取中…" : "没有拿到记忆快照"}</p>;
  }

  const lastRun = data.lastConsolidate;

  return (
    <>
      <CountBadges counts={data.counts} />

      <EventList rows={data.recentEvents} byKind={data.counts.eventsByKind} />

      <FactsPrefsList
        facts={data.recentFacts}
        preferences={data.recentPreferences}
        boundaries={boundaries}
      />

      <PredictionCard row={data.recentPredictions[0] ?? null} />

      <div className="card">
        <div className="card-head">
          <span className="card-title">上次整理</span>
          <span className="card-sub">夜间自动跑 · 也可以手动触发</span>
        </div>
        {lastRun === null ? (
          <p className="mem-empty">还没跑过整理。</p>
        ) : (
          <div className="mem-row">
            <span className="mem-kind">{lastRun.trigger}</span>
            <span className="mem-text">
              抽取 {lastRun.extracted} · 合并 {lastRun.merged} · 升级 {lastRun.promoted} ·{" "}
              {lastRun.status === "done" ? "完成" : lastRun.status === "failed" ? "失败" : "进行中"}
            </span>
            <span className="mem-ts">{formatWhen(lastRun.startedAt)}</span>
          </div>
        )}
      </div>

      <MemoryActions onChanged={() => void refresh()} />
    </>
  );
}
