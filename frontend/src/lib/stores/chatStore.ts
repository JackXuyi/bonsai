"use client";

import { create } from "zustand";
import type { CalendarRemindPayload } from "@/lib/types";

// 消息流（旧 main.js 直接操作 #messages 的 DOM，这里改为可跨组件推送的
// store——proactive / prediction / 提醒事件到达时，ChatPane 不一定可见）。
export type MessageKind = "user" | "bot" | "typing" | "prediction" | "announcement" | "reminder";

export interface ChatMessage {
  id: number;
  kind: MessageKind;
  text: string;
  /** 徽标注（badge），如过滤说明 / 主动搭话来源 */
  note?: string;
  /** bot 气泡的样式变体 */
  proactive?: boolean;
  /** 流式接收中（渲染 caret） */
  streaming?: boolean;
  prediction?: { id: number; guess: string };
  reminder?: CalendarRemindPayload;
}

interface ChatState {
  messages: ChatMessage[];
  add: (m: Omit<ChatMessage, "id">) => number;
  remove: (id: number) => void;
  /** 流式：更新指定气泡正文（找不到静默跳过） */
  appendText: (id: number, delta: string) => void;
  setText: (id: number, text: string) => void;
  setNote: (id: number, note: string) => void;
  /** 启动回放：整批灌入历史气泡（仅在消息流为空时由 ChatPane 调用） */
  prefill: (msgs: Omit<ChatMessage, "id">[]) => void;
  /** 向上分页：把更早的一页插到最前面（保持现有气泡 id 稳定） */
  prepend: (msgs: Omit<ChatMessage, "id">[]) => void;
}

let seq = 0;

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  add: (m) => {
    const id = ++seq;
    set((st) => ({ messages: [...st.messages, { ...m, id }] }));
    return id;
  },
  remove: (id) =>
    set((st) => ({ messages: st.messages.filter((m) => m.id !== id) })),
  appendText: (id, delta) =>
    set((st) => ({
      messages: st.messages.map((m) =>
        m.id === id ? { ...m, text: m.text + delta } : m
      ),
    })),
  setText: (id, text) =>
    set((st) => ({
      messages: st.messages.map((m) => (m.id === id ? { ...m, text } : m)),
    })),
  setNote: (id, note) =>
    set((st) => ({
      messages: st.messages.map((m) => (m.id === id ? { ...m, note } : m)),
    })),
  prefill: (msgs) => {
    const filled = msgs.map((m) => ({ ...m, id: ++seq }));
    set({ messages: filled });
  },
  prepend: (msgs) =>
    set((st) => {
      if (msgs.length === 0) return st;
      const filled = msgs.map((m) => ({ ...m, id: ++seq }));
      return { messages: [...filled, ...st.messages] };
    }),
}));
