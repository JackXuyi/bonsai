"use client";

// 发送 + 事件驱动的流式接收（2026-11-23 重构，P1）。
// 后端 `dialogue_chat_stream` 现在是 **fire-and-forget**：命令立即返回，整轮
// 结果通过同一 Channel 的 `turn_done` / `turn_error` 事件回传。这样一轮真的
// 跑几分钟也不会被 IPC 兜底超时误判（旧实现是「命令整轮才 resolve」）。
// 兜底改为**事件静默看门狗**：超过 IDLE_TIMEOUT_MS 没有任何事件才算断。
// 工具调用过程不进消息流（2026-11-22 裁决）：task_* 只转 planStore。
import { useCallback, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { dialogueCancel, dialogueChatStream, facePreview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { noteFor } from "@/components/face/frames";
import { useChatStore } from "@/lib/stores/chatStore";
import { usePlanStore } from "@/lib/stores/planStore";
import { toast } from "@/lib/stores/uiStore";
import type { DialogueOutcome } from "@/lib/types";

const SESSION_ID = "default";

/** 事件静默上限：一轮真实对话里最长静默是单次云端 HTTP（上限 120s），
 *  取 150s 留余量——只判「真断了」，不判「慢」。 */
export const IDLE_TIMEOUT_MS = 150000;

type StreamMsg = {
  delta?: string;
  task_step?: unknown;
  turn_done?: DialogueOutcome;
  turn_error?: string;
};

export function useChat() {
  const [chatting, setChatting] = useState(false);
  const chattingRef = useRef(false);
  /** 本轮任务 id（task_start 事件带的 id，单步任务也有）：点「停」时用 */
  const activeTaskRef = useRef<string | null>(null);
  const chat = useChatStore;

  /** 叫停本轮：后端取消标志置位，循环会在下一步边界收尾（T4）。 */
  const cancel = useCallback(async () => {
    const taskId = activeTaskRef.current;
    if (!taskId) {
      toast("还没开始执行，等它动起来再叫停");
      return;
    }
    try {
      await dialogueCancel(taskId);
      toast("已叫停");
    } catch {
      toast("叫停没生效，再看看日志");
    }
  }, []);

  const send = useCallback((raw: string) => {
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

    const plan = usePlanStore.getState();
    let bubbleId: number | null = null;
    let settled = false;
    let idle: ReturnType<typeof setTimeout> | undefined;

    const endTurn = () => {
      activeTaskRef.current = null;
      chattingRef.current = false;
      setChatting(false);
      if (idle) clearTimeout(idle);
      idle = undefined;
    };

    // 失败/断流：已流出的字一个都不撤（后端可能仍在跑，撤了就前后端各说一套）。
    const fail = (reason: unknown) => {
      store.remove(typingId);
      const tail = `\n\n（这一轮没等到结果：${reason}。它可能还在后台跑——别急着重发，稍等再看。）`;
      if (bubbleId !== null) {
        store.appendText(bubbleId, tail);
        store.patch(bubbleId, { streaming: false });
      } else {
        store.add({ kind: "bot", text: tail.trim() });
      }
    };

    const armIdle = () => {
      if (idle) clearTimeout(idle);
      idle = setTimeout(() => {
        if (settled) return;
        settled = true;
        fail(`静默超过 ${IDLE_TIMEOUT_MS / 1000} 秒`);
        endTurn();
        toast("等太久了：它可能还在后台跑");
      }, IDLE_TIMEOUT_MS);
    };

    const onDelta = new Channel<StreamMsg>();
    onDelta.onmessage = (msg) => {
      if (settled) return;
      armIdle();

      if (msg?.task_step) {
        const ev = msg.task_step as { event?: string; id?: string };
        if (ev?.event === "task_start" && typeof ev.id === "string") {
          activeTaskRef.current = ev.id;
        }
        plan.applyTaskEvent(msg.task_step);
        // 计划语义（goal + 每步 intent）只有 plan_list 有：开卡与收尾各拉一次
        // 做基线/终态对齐，执行中靠事件增量推进，不做定时轮询。
        if (
          ev?.event === "task_start" ||
          ev?.event === "task_done" ||
          ev?.event === "task_canceled"
        ) {
          void plan.refresh();
        }
        return;
      }

      // 终态事件：整轮的权威结果（P1 之后不再有「命令返回值」这条路）
      if (msg?.turn_done) {
        settled = true;
        const outcome = msg.turn_done;
        if (bubbleId === null) {
          store.remove(typingId);
          bubbleId = store.add({
            kind: "bot",
            text: "",
            ts: new Date().toISOString(),
          });
        }
        store.setText(bubbleId, outcome.reply);
        store.setNote(bubbleId, noteFor(outcome));
        if (outcome.turn_id) {
          store.patch(bubbleId, { turnId: outcome.turn_id });
          store.patch(userId, { turnId: outcome.turn_id });
        }
        facePreview("greeted").catch(() => {});
        endTurn();
        return;
      }

      if (msg?.turn_error) {
        settled = true;
        fail(msg.turn_error);
        endTurn();
        return;
      }

      if (!msg?.delta) return;
      if (bubbleId === null) {
        store.remove(typingId);
        bubbleId = store.add({
          kind: "bot",
          text: "",
          streaming: true,
          ts: new Date().toISOString(),
        });
      }
      store.appendText(bubbleId, msg.delta);
    };

    armIdle();
    // 命令只负责「受理」；结果一律走 Channel 事件。
    dialogueChatStream(text, SESSION_ID, onDelta).catch((err) => {
      if (settled) return;
      settled = true;
      fail(err);
      endTurn();
    });
  }, [chat]);

  return { send, chatting, cancel };
}
