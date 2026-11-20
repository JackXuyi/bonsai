"use client";

// 对话页（旧 tab-chat）：消息流 + 渐增气泡 + typing + 回到底部 + composer。
// 主动搭话 / 预测 / 提醒事件已上移 useAppEvents（路由化后 ChatPane 只在 /
// 挂载）；消息流状态在 chatStore，跨页保留。
import { useCallback, useEffect, useRef, useState } from "react";
import { MessageBubble } from "./MessageBubble";
import { Composer } from "./Composer";
import { useChat } from "@/hooks/useChat";
import { useChatStore, type ChatMessage } from "@/lib/stores/chatStore";
import { dialogueHistory } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";

function nearBottom(el: HTMLElement) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 56;
}

/** 把 DB 里的最近消息流映射成气泡：assistant 行若无前导 user 行即主动搭话；
 *  非今天的消息在徽标注里带日期（回放体感需要，实时流不需要）。 */
function bubblesFromHistory(
  rows: { ts: string; role: "user" | "assistant"; content: string }[]
): Omit<ChatMessage, "id">[] {
  const today = new Date().toDateString();
  const out: Omit<ChatMessage, "id">[] = [];
  let pendingUser = false;
  for (const r of rows) {
    const d = new Date(r.ts);
    const note =
      !Number.isNaN(d.getTime()) && d.toDateString() !== today
        ? `${d.getMonth() + 1}-${d.getDate()}`
        : undefined;
    if (r.role === "user") {
      pendingUser = true;
      out.push({ kind: "user", text: r.content, note });
    } else {
      const proactive = !pendingUser;
      pendingUser = false;
      out.push({ kind: "bot", text: r.content, proactive, note });
    }
  }
  return out;
}

export function ChatPane() {
  const messages = useChatStore((s) => s.messages);
  const { send, chatting } = useChat();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [showToBottom, setShowToBottom] = useState(false);

  const stick = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (nearBottom(el)) el.scrollTop = el.scrollHeight;
    syncToBottom();
  }, []);

  function syncToBottom() {
    const el = scrollRef.current;
    if (!el) return;
    setShowToBottom(!nearBottom(el) && el.scrollHeight > el.clientHeight);
  }

  // 消息变化（含流式 appendText）时贴底
  useEffect(() => {
    stick();
  }, [messages, stick]);

  // 进入对话页时聚焦输入框（旧 selectTab("chat")）
  useEffect(() => {
    scrollRef.current?.querySelector("textarea")?.focus({ preventScroll: true });
  }, []);

  // 启动回放：拉最近 50 条（旧→新）灌入消息流。仅在流为空时执行——本次
  // 启动已产生的消息优先；拉取失败静默降级（空态文案兜底）。跨启动一律以
  // DB 为准，不做本地缓存。
  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    void (async () => {
      try {
        const rows = await dialogueHistory("default", 50);
        if (
          cancelled ||
          rows.length === 0 ||
          useChatStore.getState().messages.length > 0
        )
          return;
        useChatStore.getState().prefill(bubblesFromHistory(rows));
      } catch {
        // 历史读不到就当没聊过，不挡本次对话。
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const empty = messages.length === 0;

  return (
    <>
      <div className="chat-body">
        <div className="messages" ref={scrollRef} onScroll={syncToBottom}>
          {empty && (
            <div className="empty">
              <p className="empty-title">还没聊过</p>
              <p className="empty-sub">
                说句话，或者等它自己开口。
                <br />
                它不会没话找话。
              </p>
              <div className="chips">
                <button
                  className="chip"
                  onClick={() => send("在忙什么？")}
                >
                  在忙什么？
                </button>
                <button
                  className="chip"
                  onClick={() => send("帮我记一下今天的事")}
                >
                  帮我记一下今天的事
                </button>
              </div>
            </div>
          )}
          {!empty &&
            messages.map((m: ChatMessage) => (
              <MessageBubble key={m.id} message={m} onStick={stick} />
            ))}
        </div>
        {showToBottom && (
          <button
            className="to-bottom"
            aria-label="回到最新"
            onClick={() => {
              const el = scrollRef.current;
              el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
            }}
          >
            ↓
          </button>
        )}
      </div>

      <Composer onSend={send} chatBusy={chatting} />
    </>
  );
}
