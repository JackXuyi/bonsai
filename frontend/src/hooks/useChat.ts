"use client";

// 发送 + 流式接收（旧 sendMessage：Channel onDelta 渐增气泡，typing 占位
// 首块替换；无 delta 时整段渲染；错误撤气泡）。
// 工具调用过程不进消息流（2026-11-22 用户裁决）：Channel 上的 task_* 事件
// 只转给 planStore（工具页进度面板，T6），消息只展示结果；调用明细在气泡
// 「查看详情」的 /trace 页（后端 rec.tool_call 已把每次调用落进 trace_steps）。
import { useCallback, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { dialogueChatStream, facePreview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { noteFor } from "@/components/face/frames";
import { useChatStore } from "@/lib/stores/chatStore";
import { usePlanStore } from "@/lib/stores/planStore";
import { toast } from "@/lib/stores/uiStore";

const SESSION_ID = "default";

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
    // task_step 事件改转 planStore（T6 进度面板）——仍不进消息流。
    let bubbleId: number | null = null;
    const plan = usePlanStore.getState();
    const onDelta = new Channel<{ delta?: string; task_step?: unknown }>();
    onDelta.onmessage = (msg) => {
      if (msg?.task_step) {
        plan.applyTaskEvent(msg.task_step);
        // 计划语义（goal + 每步 intent）只有 plan_list 有：开卡与收尾各拉一次
        // 做基线/终态对齐，执行中靠事件增量推进，不做定时轮询。
        const kind = (msg.task_step as { event?: string })?.event;
        if (kind === "task_start" || kind === "task_done" || kind === "task_canceled") {
          void plan.refresh();
        }
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
