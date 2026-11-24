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
import { useSessionStore } from "@/lib/stores/sessionStore";
import { toast } from "@/lib/stores/uiStore";
import type { DialogueOutcome, TurnFailure } from "@/lib/types";

/** 事件静默上限：模型层重试（plan-retry §4.2）把最长静默拉到
 *  3 次 × 120s ≈ 360s；240s 是折中——覆盖「一次超时 + 一次重试」的常见
 *  形状，又不至于让真断流的等待变得不可忍受。只判「真断了」，不判「慢」。 */
export const IDLE_TIMEOUT_MS = 240000;

/** 旧兜底句标记：取长规则用——本地已流出的字不许被它盖掉（plan-retry §7.2）。 */
const FALLBACK_MARK = "脑子转不动";

type StreamMsg = {
  delta?: string;
  task_step?: unknown;
  turn_done?: DialogueOutcome;
  /** T7：对象优先；旧字符串仍兼容。 */
  turn_error?: string | TurnFailure;
};

function turnErrorText(err: string | TurnFailure): string {
  return typeof err === "string" ? err : err.message;
}

function turnErrorFailure(err: string | TurnFailure): TurnFailure {
  if (typeof err === "string") {
    return { kind: "internal", message: err, retryable: true };
  }
  return err;
}

/** 正文取长：已流出的字只增不减；兜底句不得覆盖流式正文。 */
export function pickReplyText(local: string, outcomeReply: string): string {
  if (local.trim() && outcomeReply.includes(FALLBACK_MARK)) return local;
  return local.length > outcomeReply.length ? local : outcomeReply;
}

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

  /** 开一轮流：`reuseUserId` 有值时不插新 user 气泡（失败重试）。 */
  const beginTurn = useCallback((text: string, reuseUserId: number | null) => {
    if (!text || chattingRef.current) return;

    const store = chat.getState();
    if (!inTauri) {
      if (reuseUserId === null) {
        store.add({ kind: "user", text });
      }
      store.add({
        kind: "bot",
        text: "（浏览器预览模式：Rust 后端未连接，发消息不会有人回。）",
      });
      return;
    }

    const userId =
      reuseUserId ??
      store.add({ kind: "user", text, ts: new Date().toISOString() });
    const sessionId = useSessionStore.getState().currentId;
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
    // 读 messages 必须用 chat.getState() 新快照——beginTurn 开头缓存的
    // store.messages 在 appendText 之后是过期的。
    const fail = (reason: string | TurnFailure) => {
      store.remove(typingId);
      const f = turnErrorFailure(reason);
      const msg = turnErrorText(reason);
      if (bubbleId !== null) {
        const local =
          chat.getState().messages.find((m) => m.id === bubbleId)?.text ?? "";
        store.patch(bubbleId, {
          streaming: false,
          failure: f,
          // 无本地正文时用 hint 填；有正文只挂 failure，不覆盖。
          ...(local.trim() ? {} : { text: msg }),
        });
      } else {
        store.add({
          kind: "bot",
          text: msg,
          failure: f,
        });
      }
    };

    const armIdle = () => {
      if (idle) clearTimeout(idle);
      idle = setTimeout(() => {
        if (settled) return;
        settled = true;
        fail({
          kind: "timeout",
          message: `静默超过 ${IDLE_TIMEOUT_MS / 1000} 秒`,
          retryable: true,
        });
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
        if (
          ev?.event === "task_start" ||
          ev?.event === "task_done" ||
          ev?.event === "task_canceled"
        ) {
          void plan.refresh();
        }
        return;
      }

      if (useSessionStore.getState().currentId !== sessionId) {
        if (msg?.turn_done || msg?.turn_error) {
          settled = true;
          endTurn();
        }
        return;
      }

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
        const local =
          chat.getState().messages.find((m) => m.id === bubbleId)?.text ?? "";
        const body = pickReplyText(local, outcome.reply);
        store.setText(bubbleId, body);
        // 失败行已经说清原因，徽注不再叠「本地模型未就绪」。
        store.setNote(bubbleId, outcome.failure ? "" : noteFor(outcome));
        const patch: { turnId?: number; failure?: TurnFailure } = {};
        if (outcome.turn_id) {
          patch.turnId = outcome.turn_id;
          store.patch(userId, { turnId: outcome.turn_id });
        }
        if (outcome.failure) {
          patch.failure = outcome.failure;
        }
        if (Object.keys(patch).length) store.patch(bubbleId, patch);
        // 失败轮不装成功脸（plan-retry §7.2）。
        if (!outcome.failure) {
          facePreview("greeted").catch(() => {});
        }
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
    dialogueChatStream(text, sessionId, onDelta).catch((err) => {
      if (settled) return;
      settled = true;
      fail(String(err));
      endTurn();
    });
  }, [chat]);

  const send = useCallback(
    (raw: string) => {
      const text = raw.trim();
      if (!text) return;
      beginTurn(text, null);
    },
    [beginTurn]
  );

  /** 失败气泡「重试」：去掉失败 bot，复用上一句 user，不插第二条用户气泡。 */
  const retry = useCallback(
    (botMessageId: number) => {
      if (chattingRef.current) return;
      const msgs = chat.getState().messages;
      const idx = msgs.findIndex((m) => m.id === botMessageId);
      if (idx < 0) return;
      let userId: number | null = null;
      let userText = "";
      for (let i = idx - 1; i >= 0; i--) {
        if (msgs[i].kind === "user") {
          userId = msgs[i].id;
          userText = msgs[i].text;
          break;
        }
      }
      if (userId === null || !userText.trim()) return;
      chat.getState().remove(botMessageId);
      beginTurn(userText.trim(), userId);
    },
    [beginTurn, chat]
  );

  return { send, chatting, cancel, retry };
}
