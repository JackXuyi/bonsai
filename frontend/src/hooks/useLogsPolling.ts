"use client";

// 日志轮询（spec-m4 §4B）：页面可见时每 10s 拉一次尾部，切 tab 立即拉。
// 不可见时暂停（document.visibilityState 守卫），回到前台立即补一次。
import { useCallback, useEffect, useRef, useState } from "react";
import { debugLogTail } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import type { LogFile } from "@/lib/types";

const POLL_MS = 10_000;

export function useLogsPolling(file: LogFile, lines = 200) {
  const [rows, setRows] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const fileRef = useRef(file);
  fileRef.current = file;

  const refresh = useCallback(async () => {
    if (!inTauri) {
      setRows([]);
      setLoading(false);
      return;
    }
    try {
      const tail = await debugLogTail(fileRef.current, lines);
      setRows(tail);
    } catch {
      setRows([]); // 拉失败 = 暂无日志，不打扰
    } finally {
      setLoading(false);
    }
  }, [lines]);

  // 切 tab 立即拉一次
  useEffect(() => {
    void refresh();
  }, [refresh, file]);

  // 可见时 10s 轮询
  useEffect(() => {
    if (!inTauri) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh();
    }, POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh]);

  return { rows, loading, refresh };
}
