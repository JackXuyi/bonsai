"use client";

// ⌘1–⌘5 跳转路由（spec-m0b §1）。映射逻辑在 lib/shortcut.ts（纯函数带测）。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { routeForKeys } from "@/lib/shortcut";

export function useTabShortcuts() {
  const router = useRouter();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const path = routeForKeys(e.metaKey || e.ctrlKey, e.key);
      if (path) {
        e.preventDefault();
        router.push(path);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [router]);
}
