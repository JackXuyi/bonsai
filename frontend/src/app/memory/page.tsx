"use client";

// 旧 /memory → 设置·记忆（plan-chat-first-ui v2）。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { settingsHref } from "@/lib/settingsSections";

export default function MemoryRedirectPage() {
  const router = useRouter();
  useEffect(() => {
    router.replace(settingsHref("memory"));
  }, [router]);
  return null;
}
