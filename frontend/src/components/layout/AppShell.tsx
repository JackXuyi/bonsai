"use client";

// Companion 壳（plan-chat-first-ui v2）：单列内容区；无功能侧栏。
// Onboarding / Toast / 全局事件 / 快捷键在壳层挂载。
// 授权：聊天页由 Composer dock 挂 InlineApprovalBar；其它路由在此挂 toast 条。
import { OnboardingFlow } from "@/components/onboarding/OnboardingFlow";
import { ToastHost } from "@/components/ToastHost";
import { InlineApprovalBar } from "@/components/agent/InlineApprovalBar";
import { usePathname } from "next/navigation";
import { useEffect } from "react";
import { useAppEvents } from "@/hooks/useAppEvents";
import { useAppShortcuts } from "@/hooks/useAppShortcuts";
import { useLlmStore } from "@/lib/stores/llmStore";

export function AppShell({ children }: { children: React.ReactNode }) {
  useAppEvents();
  useAppShortcuts();
  const pathname = usePathname();
  const onChat = pathname === "/";

  useEffect(() => {
    void useLlmStore.getState().load();
  }, []);

  return (
    <main className="app">
      <div className="content">
        <div className="page">{children}</div>
      </div>
      <OnboardingFlow />
      <ToastHost />
      {!onChat ? <InlineApprovalBar placement="toast" /> : null}
    </main>
  );
}
