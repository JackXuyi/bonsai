"use client";

// Companion 壳（plan-chat-first-ui v2）：单列内容区；无功能侧栏。
// Onboarding / Toast / Approval / 全局事件 / 快捷键在壳层挂载。
import { OnboardingFlow } from "@/components/onboarding/OnboardingFlow";
import { ToastHost } from "@/components/ToastHost";
import { ApprovalCard } from "@/components/agent/ApprovalCard";
import { useEffect } from "react";
import { useAppEvents } from "@/hooks/useAppEvents";
import { useAppShortcuts } from "@/hooks/useAppShortcuts";
import { useLlmStore } from "@/lib/stores/llmStore";

export function AppShell({ children }: { children: React.ReactNode }) {
  useAppEvents();
  useAppShortcuts();

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
      <ApprovalCard />
    </main>
  );
}
