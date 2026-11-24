"use client";

// 链路追踪入口（spec-s8）：/trace?turn= / ?event=；Companion 二级顶栏。
import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { TraceView } from "@/components/trace/TraceView";
import { SecondaryChrome } from "@/components/layout/SecondaryChrome";

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
    <>
      <SecondaryChrome title="生成链路" />
      <div className="tab-body">
        <Suspense fallback={<p className="hint-line trace-empty">链路读取中…</p>}>
          <TraceByQuery />
        </Suspense>
      </div>
    </>
  );
}
