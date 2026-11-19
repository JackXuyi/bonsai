"use client";

// 对话页（旧 tab-chat）：消息流 + 渐增气泡 + typing + 回到底部 + composer。
// 主动搭话 / 预测 / 提醒事件已上移 useAppEvents（路由化后 ChatPane 只在 /
// 挂载）；消息流状态在 chatStore，跨页保留。
import { useCallback, useEffect, useRef, useState } from "react";
import { MessageBubble } from "./MessageBubble";
import { Composer } from "./Composer";
import { useChat } from "@/hooks/useChat";
import { useChatStore, type ChatMessage } from "@/lib/stores/chatStore";

function nearBottom(el: HTMLElement) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 56;
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
