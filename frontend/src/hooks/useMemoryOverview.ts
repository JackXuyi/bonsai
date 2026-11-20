"use client";

// 记忆总览数据编排（spec-m3 §2）：进页拉取 + 手动刷新。
// 浏览器预览（无 Tauri）下不调用命令——空白快照比一堆红字有用地多。
import { useCallback, useEffect, useState } from "react";
import { boundaryList, memoryOverview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import type { Boundary, MemoryOverview } from "@/lib/types";

/** 空快照：预览态与首次加载共用一个形状，UI 不必到处判 null。 */
export function emptyOverview(): MemoryOverview {
  return {
    counts: {
      events: 0,
      eventsByKind: [],
      facts: 0,
      preferences: 0,
      boundaries: 0,
      predictionsTotal: 0,
      predictionsHit: 0,
      toolCalls: 0,
      calendarCache: 0,
    },
    recentEvents: [],
    recentFacts: [],
    recentPreferences: [],
    recentPredictions: [],
    lastConsolidate: null,
  };
}

export function useMemoryOverview(limit = 20) {
  const [data, setData] = useState<MemoryOverview | null>(null);
  // 边界走既有 boundary_list（spec-m3 §2：不新增读取路径，删除入口仍只在工具页）
  const [boundaries, setBoundaries] = useState<Boundary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setData(emptyOverview());
      setBoundaries([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const [overview, lines] = await Promise.all([memoryOverview(limit), boundaryList()]);
      setData(overview);
      setBoundaries(lines);
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [limit]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { data, boundaries, loading, error, refresh };
}
