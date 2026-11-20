"use client";

// 记忆 tab（spec-m3）：只读总览。记忆可看是信任，可改是作弊。
import { MemoryOverview } from "@/components/memory/MemoryOverview";

export default function MemoryPage() {
  return (
    <div className="tab-body">
      <MemoryOverview />
    </div>
  );
}
