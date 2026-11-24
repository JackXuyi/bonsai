"use client";

// 调用记录卡（spec-s6 §3）：运维速览，复用完整审计时间线的 compact 形态。
import { AuditTimeline } from "@/components/agent/AuditTimeline";

export function ToolCallRecords() {
  return <AuditTimeline variant="compact" />;
}
