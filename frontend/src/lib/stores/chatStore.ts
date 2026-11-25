"use client";

import { create } from "zustand";
import type { CalendarRemindPayload, TurnFailure } from "@/lib/types";

// 消息流（旧 main.js 直接操作 #messages 的 DOM，这里改为可跨组件推送的
// store——proactive / prediction / 提醒事件到达时，ChatPane 不一定可见）。
// 工具原文不进对话正文；授权条与计划进度可进（plan-chat-agent-inline）。
// 调用明细仍走气泡「查看详情」/trace（后端 trace_steps 落库）。
export type MessageKind =
  | "user"
  | "bot"
  | "typing"
  | "prediction"
  | "announcement"
  | "reminder"
  | "plan";

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
  /** kind=plan：只存 task id，进度权威在 planStore */
  planTaskId?: string;
  /** 消息时间（ISO；实时路径发送/落库时记录，回放路径取 events.ts） */
  ts?: string;
  /** events 表 id（spec-s8）：历史回放直接带，实时路径落库后补 */
  eventId?: number;
  /** 链路追踪回合 id（spec-s8）：气泡「查看详情」入口 */
  turnId?: number;
  /** 本轮失败（plan-retry T7）：细行 + 可选「重试」 */
  failure?: TurnFailure;
}

interface ChatState {
  messages: ChatMessage[];
  add: (m: Omit<ChatMessage, "id">) => number;
  remove: (id: number) => void;
  /** 流式：更新指定气泡正文（找不到静默跳过） */
  appendText: (id: number, delta: string) => void;
  setText: (id: number, text: string) => void;
  setNote: (id: number, note: string) => void;
  /** 局部补丁（spec-s8）：落库后回填 turnId / eventId / ts，不触发整条替换 */
  patch: (id: number, partial: Partial<ChatMessage>) => void;
  /** 启动回放：整批灌入历史气泡（仅在消息流为空时由 ChatPane 调用） */
  prefill: (msgs: Omit<ChatMessage, "id">[]) => void;
  /** 切会话：整条流清空（旧会话的气泡一个不留，A12）；id 序号不回退，
   *  防止旧 key 被新气泡复用导致 React 误复用节点。 */
  reset: () => void;
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
  /** 正文落定：流式气泡收尾（spec-s8 修复——streaming 只在此处清除，
   *  否则操作条 `!message.streaming` 永远不渲染） */
  setText: (id, text) =>
    set((st) => ({
      messages: st.messages.map((m) =>
        m.id === id ? { ...m, text, streaming: false } : m
      ),
    })),
  setNote: (id, note) =>
    set((st) => ({
      messages: st.messages.map((m) => (m.id === id ? { ...m, note } : m)),
    })),
  patch: (id, partial) =>
    set((st) => ({
      messages: st.messages.map((m) => (m.id === id ? { ...m, ...partial } : m)),
    })),
  prefill: (msgs) => {
    const filled = msgs.map((m) => ({ ...m, id: ++seq }));
    set({ messages: filled });
  },
  reset: () => set({ messages: [] }),
  prepend: (msgs) =>
    set((st) => {
      if (msgs.length === 0) return st;
      const filled = msgs.map((m) => ({ ...m, id: ++seq }));
      return { messages: [...filled, ...st.messages] };
    }),
}));
