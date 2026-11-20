"use client";

// 页头（spec-m0b §1）：当前页标题 + 右侧状态摘要
// （llm:ready → 「本地 · qwen3.5:2b」；M1/M2 扩展 provider 文案）。
import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { titleForPath } from "@/lib/shortcut";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";

export function PageHeader() {
  const pathname = usePathname();
  const llmModel = useUiStore((s) => s.llmModel);

  // inTauri 是模块加载期常量：预渲染时 window 不存在（false），Tauri 客户端水合时是 true，
  // 直接用它决定文本会触发 hydration mismatch。首帧固定渲染与预渲染一致的「浏览器预览」，
  // 挂载后再切真实值。
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  const tauri = mounted && inTauri;

  const status = llmModel
    ? `本地 · ${llmModel}`
    : tauri
      ? "模型连接中…"
      : "浏览器预览";

  return (
    <header className="page-head">
      <h1 className="page-title">{titleForPath(pathname)}</h1>
      <span className="page-status">{status}</span>
    </header>
  );
}
