"use client";

// 旧 /tools → 设置·权限与工具（plan-chat-first-ui v2）。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { settingsHref } from "@/lib/settingsSections";

export default function ToolsRedirectPage() {
  const router = useRouter();
  useEffect(() => {
    router.replace(settingsHref("access"));
  }, [router]);
  return null;
}
