"use client";

import { create } from "zustand";
import { FACE_STATES, type FaceState, type GrowthStatus, type RelationshipStatus } from "@/lib/types";

export interface Toast {
  id: number;
  text: string;
}

export interface ToolResult {
  title: string;
  text?: string;
  pairs?: [string, string][];
}

const TOAST_CAP = 4;
const TOAST_VISIBLE_MS = 1900;

interface UiState {
  faceState: FaceState;
  growth: GrowthStatus | null;
  relationship: RelationshipStatus | null;
  /** 表情页形态演示的临时覆盖（null = 用真实 growth） */
  stagePreview: { stage: number; dormant: boolean } | null;
  llmModel: string | null;
  toolResult: ToolResult | null;
  /** window:shown 断点计数（托盘唤起 / growth 播报重引导的触发器） */
  windowShownAt: number;
  toasts: Toast[];
  setFaceState: (s: string) => void;
  setGrowth: (g: GrowthStatus | null) => void;
  setRelationship: (r: RelationshipStatus | null) => void;
  setStagePreview: (p: { stage: number; dormant: boolean } | null) => void;
  setLlmModel: (m: string) => void;
  setToolResult: (r: ToolResult | null) => void;
  bumpWindowShown: () => void;
  pushToast: (text: string) => void;
  dismissToast: (id: number) => void;
}

let toastSeq = 0;
let toastTimer: ReturnType<typeof setTimeout> | undefined;

export const useUiStore = create<UiState>((set) => ({
  faceState: "calm",
  growth: null,
  relationship: null,
  stagePreview: null,
  llmModel: null,
  toolResult: null,
  windowShownAt: 0,
  toasts: [],
  setFaceState: (s) => {
    if (!FACE_STATES.includes(s as FaceState)) return;
    set({ faceState: s as FaceState });
  },
  setGrowth: (g) => set({ growth: g, stagePreview: null }),
  setRelationship: (r) => set({ relationship: r }),
  setStagePreview: (p) => set({ stagePreview: p }),
  setLlmModel: (m) => set({ llmModel: m }),
  setToolResult: (r) => set({ toolResult: r }),
  bumpWindowShown: () => set((st) => ({ windowShownAt: st.windowShownAt + 1 })),
  pushToast: (text) => {
    const id = ++toastSeq;
    set((st) => ({ toasts: [...st.toasts, { id, text }].slice(-TOAST_CAP) }));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(
      () => set((st) => ({ toasts: st.toasts.filter((t) => t.id !== id) })),
      TOAST_VISIBLE_MS
    );
  },
  dismissToast: (id) =>
    set((st) => ({ toasts: st.toasts.filter((t) => t.id !== id) })),
}));

/** 模块级 toast（等价旧 main.js 的全局 toast()）。 */
export function toast(text: string) {
  useUiStore.getState().pushToast(text);
}
