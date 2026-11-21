"use client";

// 链路追踪入口（spec-s8 S8-d）：/trace?turn={id} 直取主档；
// /trace?event={eventId} 先反查 turn（历史气泡的懒查入口）。
// 静态导出（output: export）不支持动态路由段，参数一律走 query；
// useSearchParams 在预渲染期需要 Suspense 边界包裹。
import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { TraceView } from "@/components/trace/TraceView";

function TraceByQuery() {
  const sp = useSearchParams();
  const turnId = Number(sp.get("turn"));
  const eventId = Number(sp.get("event"));
  return (
    <TraceView
      turnId={Number.isFinite(turnId) && turnId > 0 ? turnId : undefined}
      eventId={Number.isFinite(eventId) && eventId > 0 ? eventId : undefined}
    />
  );
}

export default function TraceIndexPage() {
  return (
    <div className="tab-body">
      <Suspense fallback={<p className="hint-line trace-empty">链路读取中…</p>}>
        <TraceByQuery />
      </Suspense>
    </div>
  );
}
