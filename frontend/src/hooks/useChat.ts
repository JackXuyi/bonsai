"use client";

// 发送 + 流式接收（旧 sendMessage：Channel onDelta 渐增气泡，typing 占位
// 首块替换；无 delta 时整段渲染；错误撤气泡）。
// 任务卡（v12）：同一 Channel 上的 task_step 结构化消息——分流渲染任务卡，
// 不进正文气泡；终态以 DialogueOutcome.task 为准。
import { useCallback, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { dialogueChatStream, facePreview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { noteFor } from "@/components/face/frames";
import { useChatStore } from "@/lib/stores/chatStore";
import { toast } from "@/lib/stores/uiStore";
import type { TaskEvent, TaskInfo } from "@/lib/types";

const SESSION_ID = "default";

/** 任务卡事件 → 消息流（实时进度；找不到卡片时按事件类型补建）。 */
function applyTaskEvent(ev: TaskEvent) {
  const store = useChatStore.getState();
  const existing = store.messages.find((m) => m.task?.id === ev.id);
  if (!existing) {
    // 漏了 task_start（如重连）：按当前事件最小补建。
    const task: TaskInfo =
      ev.event === "task_step"
        ? { id: ev.id, status: "running", steps: [{ seq: ev.seq, tool: ev.tool, status: ev.status }] }
        : { id: ev.id, status: ev.event === "task_done" ? ev.status : "running", steps: [] };
    store.add({ kind: "task", text: "", task });
    return;
  }
  if (ev.event === "task_step") {
    const steps = existing.task!.steps.some((s) => s.seq === ev.seq)
      ? existing.task!.steps
      : [...existing.task!.steps, { seq: ev.seq, tool: ev.tool, status: ev.status }];
    store.patch(existing.id, { task: { ...existing.task!, steps } });
  } else if (ev.event === "task_done") {
    store.patch(existing.id, { task: { ...existing.task!, status: ev.status } });
  }
}

export function useChat() {
  const [chatting, setChatting] = useState(false);
  const chattingRef = useRef(false);
  const chat = useChatStore;

  const send = useCallback(async (raw: string) => {
    const text = raw.trim();
    if (!text || chattingRef.current) return;

    const store = chat.getState();
    if (!inTauri) {
      store.add({ kind: "user", text });
      store.add({
        kind: "bot",
        text: "（浏览器预览模式：Rust 后端未连接，发消息不会有人回。）",
      });
      return;
    }

    const userId = store.add({ kind: "user", text, ts: new Date().toISOString() });
    chattingRef.current = true;
    setChatting(true);
    const typingId = store.add({ kind: "typing", text: "" });

    // Streaming: deltas arrive while the model is still generating; the
    // typing placeholder is swapped for a real bubble on the first chunk.
    // task_step 事件分流到任务卡（同一 Channel，不进正文气泡）。
    let bubbleId: number | null = null;
    const onDelta = new Channel<{ delta?: string; task_step?: TaskEvent }>();
    onDelta.onmessage = (msg) => {
      if (msg?.task_step) {
        applyTaskEvent(msg.task_step);
        return;
      }
      if (!msg?.delta) return;
      if (bubbleId === null) {
        store.remove(typingId);
        bubbleId = store.add({ kind: "bot", text: "", streaming: true, ts: new Date().toISOString() });
      }
      store.appendText(bubbleId, msg.delta);
    };

    try {
      const outcome = await dialogueChatStream(text, SESSION_ID, onDelta);
      const finishedAt = new Date().toISOString();
      if (bubbleId === null) {
        // No deltas at all (fallback line / dead model) — render whole reply.
        store.remove(typingId);
        bubbleId = store.add({ kind: "bot", text: "", ts: finishedAt });
      }
      store.setText(bubbleId, outcome.reply);
      store.setNote(bubbleId, noteFor(outcome));
      // Trace linkage (spec-s8): both sides of the turn link to the trace page.
      if (outcome.turn_id) {
        store.patch(bubbleId, { turnId: outcome.turn_id });
        store.patch(userId, { turnId: outcome.turn_id });
      }
      // 任务卡终态对齐（v12）：实时事件可能漏步，终态以后端 outcome 为准。
      if (outcome.task?.id) {
        const existing = store.messages.find((m) => m.task?.id === outcome.task!.id);
        if (existing) {
          store.patch(existing.id, { task: outcome.task });
        } else {
          store.add({ kind: "task", text: "", task: outcome.task, ts: finishedAt });
        }
      }
      // Speaking counts as "being talked to": flash the greeted face.
      facePreview("greeted").catch(() => {});
    } catch (err) {
      if (bubbleId !== null) store.remove(bubbleId);
      store.remove(typingId);
      store.add({ kind: "bot", text: `（出错了：${err}）` });
      toast("没发出去，再看看日志");
    } finally {
      chattingRef.current = false;
      setChatting(false);
    }
  }, [chat]);

  return { send, chatting };
}
