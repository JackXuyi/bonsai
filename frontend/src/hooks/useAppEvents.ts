"use client";

// 应用级事件接线（旧 main.js 的 boot + listen 段，M0b 起全部路由共用）：
// face:state / face_current、growth:stage、relationship:stage、window:shown、
// llm:ready，以及主动搭话 / 预测 / 提醒三类（原在 ChatPane 内，路由化后
// ChatPane 只在 / 挂载，事件必须上移到壳层；到达时 router.push("/")）。
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { faceCurrent, facePreview } from "@/lib/commands";
import {
  drainAbilityAnnouncements,
  drainGrowthAnnouncements,
  drainRelationshipAnnouncements,
} from "@/lib/announcements";
import { useTauriEvent } from "@/lib/tauriListen";
import { inTauri } from "@/lib/tauriInvoke";
import { useChatStore } from "@/lib/stores/chatStore";
import { useLlmStore } from "@/lib/stores/llmStore";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type {
  CalendarRemindPayload,
  PredictionAskedPayload,
} from "@/lib/types";

// 浏览器预览兜底：本地演示轮换（旧 Hero else 分支，壳层单实例）
const DEMO_CYCLE = ["calm", "focused", "calm", "thinking", "calm", "sleepy"];

export function useAppEvents() {
  const router = useRouter();
  const setFaceState = useUiStore((s) => s.setFaceState);

  useTauriEvent<string>("face:state", (p) => setFaceState(p));
  useTauriEvent<unknown>("growth:stage", () => {
    drainGrowthAnnouncements().catch(() => {});
  });
  useTauriEvent<unknown>("relationship:stage", () => {
    drainRelationshipAnnouncements().catch(() => {});
  });
  useTauriEvent<unknown>("ability:stage", () => {
    drainAbilityAnnouncements().catch(() => {});
  });
  // sidecar 起来了 → 补一次配置读取（页头状态依赖 provider / activeId）。
  useTauriEvent<unknown>("llm:ready", () => {
    void useLlmStore.getState().load();
  });
  useTauriEvent<unknown>("window:shown", () => {
    // tray「重新做一次引导」等：重查 growth/关系播报与 onboarding
    drainGrowthAnnouncements().catch(() => {});
    drainRelationshipAnnouncements().catch(() => {});
    drainAbilityAnnouncements().catch(() => {});
    useUiStore.getState().bumpWindowShown();
  });

  // 主动搭话：插气泡 + 切到对话页（原 ChatPane proactive:message）。
  // turn_id（spec-s8）：气泡「查看详情」直达该回合的生成链路。
  useTauriEvent<{ text: string; turn_id?: number }>("proactive:message", (p) => {
    useChatStore.getState().add({
      kind: "bot",
      text: p.text,
      proactive: true,
      turnId: p.turn_id,
      ts: new Date().toISOString(),
      note: "它自己开口的",
    });
    router.push("/");
    toast("它说了句什么");
    facePreview("greeted").catch(() => {});
  });

  // 预测卡（原 ChatPane prediction:asked）
  useTauriEvent<PredictionAskedPayload>("prediction:asked", (p) => {
    useChatStore
      .getState()
      .add({
        kind: "prediction",
        text: p.guess,
        prediction: { id: p.id, guess: p.guess },
      });
    router.push("/");
    toast("它猜了一件事");
  });

  // 提醒已由 Rust 去重限频；隐藏窗口保持沉默（不是通知中心）
  useTauriEvent<CalendarRemindPayload>("calendar:remind", (p) => {
    if (document.visibilityState === "hidden") return;
    useChatStore.getState().add({ kind: "reminder", text: p.title, reminder: p });
    router.push("/");
  });

  useEffect(() => {
    if (!inTauri) {
      let i = 0;
      const t = setInterval(() => {
        i = (i + 1) % DEMO_CYCLE.length;
        setFaceState(DEMO_CYCLE[i]);
      }, 6000);
      return () => clearInterval(t);
    }
    faceCurrent().then(setFaceState).catch(() => {});
    drainGrowthAnnouncements().catch(() => {});
    drainRelationshipAnnouncements().catch(() => {});
    drainAbilityAnnouncements().catch(() => {});
  }, [setFaceState]);
}
