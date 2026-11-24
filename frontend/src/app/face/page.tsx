"use client";

// 旧 /face → 设置·形象（plan-chat-first-ui v2）。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { settingsHref } from "@/lib/settingsSections";

export default function FaceRedirectPage() {
  const router = useRouter();
  useEffect(() => {
    router.replace(settingsHref("presence"));
  }, [router]);
  return null;
}
