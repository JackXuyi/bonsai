"use client";

// 页头（spec-m0b §1）：当前页标题 + 右侧状态摘要
// （llm:ready → 「本地 · qwen3.5:2b」；M1/M2 扩展 provider 文案）。
import { usePathname } from "next/navigation";
import { titleForPath } from "@/lib/shortcut";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";

export function PageHeader() {
  const pathname = usePathname();
  const llmModel = useUiStore((s) => s.llmModel);

  const status = llmModel
    ? `本地 · ${llmModel}`
    : inTauri
      ? "模型连接中…"
      : "浏览器预览";

  return (
    <header className="page-head">
      <h1 className="page-title">{titleForPath(pathname)}</h1>
      <span className="page-status">{status}</span>
    </header>
  );
}
