"use client";

// Toast 宿主（旧 toast() 的单元素展示：最新一条，1.9s 自动消失）。
import { useUiStore } from "@/lib/stores/uiStore";

export function ToastHost() {
  const toasts = useUiStore((s) => s.toasts);
  const latest = toasts[toasts.length - 1];

  return (
    <div
      className={`toast${latest ? " show" : ""}`}
      role="status"
      aria-live="polite"
    >
      {latest?.text ?? ""}
    </div>
  );
}
