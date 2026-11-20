"use client";

// 页头（spec-m0b §1）：当前页标题 + 右侧状态摘要。
// 状态源是 llmStore（provider + activeId）：本地显绿点，云端显黄点（spec-m2 §3）。
import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { normalizePath, titleForPath } from "@/lib/shortcut";
import { inTauri } from "@/lib/tauriInvoke";
import { useLlmStore } from "@/lib/stores/llmStore";

export function PageHeader() {
  const pathname = normalizePath(usePathname());
  const loaded = useLlmStore((s) => s.loaded);
  const provider = useLlmStore((s) => s.provider);
  const activeId = useLlmStore((s) => s.activeId);
  const localModel = useLlmStore((s) => s.localModel);
  const cloudEntries = useLlmStore((s) => s.cloudEntries);

  // inTauri 是模块加载期常量：预渲染时 window 不存在（false），Tauri 客户端水合时是 true，
  // 直接用会触发 hydration mismatch。首帧固定渲染与预渲染一致的「浏览器预览」，挂载后再切真实值。
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  const tauri = mounted && inTauri;

  let status = tauri ? "模型连接中…" : "浏览器预览";
  let tone = "pending";
  if (loaded) {
    if (provider === "cloud") {
      const entry = cloudEntries.find((e) => `cloud:${e.id}` === activeId);
      status = `云端 · ${entry?.name ?? "未选择"}`;
      tone = "cloud";
    } else {
      status = `本地 · ${localModel}`;
      tone = "local";
    }
  }

  return (
    <header className="page-head">
      <h1 className="page-title">{titleForPath(pathname)}</h1>
      <span className={`page-status ${tone}`}>
        <span className="dot" aria-hidden="true" />
        {status}
      </span>
    </header>
  );
}
