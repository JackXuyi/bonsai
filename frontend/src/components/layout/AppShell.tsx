"use client";

// 应用壳（spec-m0b §1）：.app 改 Grid 两栏（92px 侧栏 + 内容区），
// 侧栏 + 页头跨路由常驻；OnboardingFlow / ToastHost / 全局事件在壳层挂载。
import { Sidebar } from "@/components/layout/Sidebar";
import { PageHeader } from "@/components/layout/PageHeader";
import { OnboardingFlow } from "@/components/onboarding/OnboardingFlow";
import { ToastHost } from "@/components/ToastHost";
import { ApprovalCard } from "@/components/agent/ApprovalCard";
import { useEffect } from "react";
import { useAppEvents } from "@/hooks/useAppEvents";
import { useTabShortcuts } from "@/hooks/useTabShortcuts";
import { useLlmStore } from "@/lib/stores/llmStore";

export function AppShell({ children }: { children: React.ReactNode }) {
  useAppEvents();
  useTabShortcuts();

  // 页头状态依赖 provider / activeId —— 壳层只读一次（load 自身幂等）。
  useEffect(() => {
    void useLlmStore.getState().load();
  }, []);

  return (
    <main className="app">
      <Sidebar />
      <div className="content">
        <PageHeader />
        <div className="page">{children}</div>
      </div>
      <OnboardingFlow />
      <ToastHost />
      <ApprovalCard />
    </main>
  );
}
