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

  // —— 分页回放状态（refs：不参与渲染，避免 onScroll 高频重渲） ——
  const PAGE_SIZE = 30;
  const SCROLL_TOP_TRIGGER = 80;
  const oldestIdRef = useRef<number | null>(null); // 已加载最旧一行的 db id（游标）
  const hasMoreRef = useRef(true);
  const loadingRef = useRef(false);
  /** 前插历史页时记录 (height, top)，渲染后按差值复位，视口不跳动。 */
  const restoreRef = useRef<{ prevHeight: number; prevTop: number } | null>(null);
  /** 首次有内容时直接跳到底部（prefill 后 nearBottom 条件会误判停在顶部）。 */
  const jumpedRef = useRef(false);

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

  // 消息变化时的滚动策略：前插历史页 → 按差值复位；首次有内容 → 跳到最新；
  // 其余（新消息 / 流式）→ 原有贴底逻辑。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || messages.length === 0) return;
    if (restoreRef.current) {
      const { prevHeight, prevTop } = restoreRef.current;
      restoreRef.current = null;
      el.scrollTop = el.scrollHeight - prevHeight + prevTop;
      syncToBottom();
    } else if (!jumpedRef.current) {
      jumpedRef.current = true;
      el.scrollTop = el.scrollHeight;
      syncToBottom();
    } else {
      stick();
    }
  }, [messages, stick]);

  // 向上分页：滚近顶部时拉更早的一页（游标 = 已加载最旧的 db id）。
  const loadOlder = useCallback(async () => {
    if (!inTauri || loadingRef.current || !hasMoreRef.current) return;
    const before = oldestIdRef.current;
    if (before === null) return;
    loadingRef.current = true;
    try {
      const rows = await dialogueHistory("default", PAGE_SIZE, before);
      if (rows.length === 0) {
        hasMoreRef.current = false;
        return;
      }
      if (rows.length < PAGE_SIZE) hasMoreRef.current = false;
      oldestIdRef.current = rows[0].id;
      const el = scrollRef.current;
      if (el) restoreRef.current = { prevHeight: el.scrollHeight, prevTop: el.scrollTop };
      useChatStore.getState().prepend(bubblesFromHistory(rows));
    } catch {
      hasMoreRef.current = false; // 读不到更早的就到此为止，不打扰对话
    } finally {
      loadingRef.current = false;
    }
  }, []);

  function onMessagesScroll() {
    syncToBottom();
    const el = scrollRef.current;
    if (el && el.scrollTop <= SCROLL_TOP_TRIGGER) void loadOlder();
  }

  // 进入对话页时聚焦输入框（旧 selectTab("chat")）
  useEffect(() => {
    scrollRef.current?.querySelector("textarea")?.focus({ preventScroll: true });
  }, []);

  // 启动回放：只拉第一页（30 条），其余滚到顶部再分页加载。仅在流为空时
  // 执行——本次启动已产生的消息优先；拉取失败静默降级（空态文案兜底）。
  // 跨启动一律以 DB 为准，不做本地缓存。
  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    void (async () => {
      try {
        const rows = await dialogueHistory("default", PAGE_SIZE);
        if (
          cancelled ||
          rows.length === 0 ||
          useChatStore.getState().messages.length > 0
        )
          return;
        oldestIdRef.current = rows[0].id;
        hasMoreRef.current = rows.length === PAGE_SIZE;
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
        <div className="messages" ref={scrollRef} onScroll={onMessagesScroll}>
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
