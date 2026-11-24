"use client";

// Companion 快捷键（plan-chat-first-ui v2）：⌘, 设置；⌘⇧O 会话抽屉。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { shortcutAction } from "@/lib/shortcut";
import { useUiStore } from "@/lib/stores/uiStore";

export function useAppShortcuts() {
  const router = useRouter();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const action = shortcutAction(
        e.metaKey || e.ctrlKey,
        e.shiftKey,
        e.key
      );
      if (!action) return;
      e.preventDefault();
      if (action.kind === "route") {
        router.push(action.path);
        return;
      }
      useUiStore.getState().toggleSessionDrawer();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [router]);
}
