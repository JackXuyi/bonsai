"use client";

// 最近一次预测（spec-m3 §2）：只读形态——反馈按钮只在对话页出现，
// 记忆页看到的是「它猜了什么、结果如何」，不是让你在这里改判。
import type { MemoryPredictionRow } from "@/lib/types";

const FEEDBACK_LABEL: Record<string, string> = {
  hit: "猜对了",
  miss: "猜错了",
  shut_up: "让你别烦",
};

const SKIP_LABEL: Record<string, string> = {
  no_candidate: "没找到可猜的",
  busy: "你在开会",
  spoke_today: "今天已经聊过",
  backoff: "这类猜错太多，暂时停",
};

/** 一句话状态：待反馈 / 已反馈 / 没问 + 原因。 */
export function predictionStatus(row: MemoryPredictionRow): string {
  if (row.feedback && FEEDBACK_LABEL[row.feedback]) return FEEDBACK_LABEL[row.feedback];
  if (!row.askedAt) return SKIP_LABEL[row.skipReason ?? ""] ?? "没问";
  return "待反馈（48 小时内有效）";
}

export function PredictionCard({ row }: { row: MemoryPredictionRow | null }) {
  return (
    <div className="card">
      <div className="card-head">
        <span className="card-title">最近一次预测</span>
        <span className="card-sub">预测-验证循环</span>
      </div>
      {row === null ? (
        <p className="mem-empty">还没有预测。它还在学你的作息。</p>
      ) : (
        <div className="mem-row">
          <span className="mem-kind">prediction</span>
          <span className="mem-text">「{row.guess}」</span>
          <span className={`mem-ts ${row.feedback ? "settled" : ""}`}>
            {predictionStatus(row)} · {row.date}
          </span>
        </div>
      )}
    </div>
  );
}
