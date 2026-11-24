"use client";

// Companion 对话页（plan-chat-first-ui v2）：ChatChrome + 消息流 + 浮起
// Composer + 左会话抽屉。主动搭话等事件在 useAppEvents；消息在 chatStore。
import { useCallback, useEffect, useRef, useState } from "react";
import { MessageBubble } from "./MessageBubble";
import { Composer } from "./Composer";
import { SessionDrawer } from "./SessionDrawer";
import { ChatChrome } from "@/components/layout/ChatChrome";
import { PokeFace } from "@/components/face/PokeFace";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import { useChat } from "@/hooks/useChat";
import { useChatStore, type ChatMessage } from "@/lib/stores/chatStore";
import { useSessionStore } from "@/lib/stores/sessionStore";
import { useUiStore } from "@/lib/stores/uiStore";
import { dialogueHistory } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";

function nearBottom(el: HTMLElement) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 56;
}

/** 空态大脸：单独挂 useFaceFrame，避免有消息时整页 600ms 重渲。 */
function EmptyStateFace() {
  const frame = useFaceFrame();
  return (
    <div className="empty-face">
      <PokeFace size="lg" frame={frame} />
    </div>
  );
}

/** 把 DB 里的最近消息流映射成气泡：assistant 行若无前导 user 行即主动搭话。
 *  时间不进气泡体（2026-11-23 用户裁决）：日期只在操作条的 msg-time 里。
 *  eventId / ts 透传（spec-s8）：操作条的时间显示与「查看详情」反查入口。 */
function bubblesFromHistory(
  rows: { id: number; ts: string; role: "user" | "assistant"; content: string }[]
): Omit<ChatMessage, "id">[] {
  const out: Omit<ChatMessage, "id">[] = [];
  let pendingUser = false;
  for (const r of rows) {
    if (r.role === "user") {
      pendingUser = true;
      out.push({ kind: "user", text: r.content, ts: r.ts, eventId: r.id });
    } else {
      const proactive = !pendingUser;
      pendingUser = false;
      out.push({ kind: "bot", text: r.content, proactive, ts: r.ts, eventId: r.id });
    }
  }
  return out;
}

export function ChatPane() {
  const messages = useChatStore((s) => s.messages);
  const { send, chatting, cancel, retry } = useChat();
  const currentId = useSessionStore((s) => s.currentId);
  const drawerOpen = useUiStore((s) => s.sessionDrawerOpen);
  const setDrawerOpen = useUiStore((s) => s.setSessionDrawerOpen);
  const closeDrawer = useCallback(() => setDrawerOpen(false), [setDrawerOpen]);
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
  /** 用户是否贴底：贴底时流式增量涨多少都跟着滚；用户往上翻就别打扰。 */
  const pinnedRef = useRef(true);
  /**
   * 程序化改 scrollTop 期间忽略 onScroll 里的 setShowToBottom。
   * WebKit 可在 effect/rAF 写 scrollTop 时同步派发 scroll；若立刻 setState，
   * 会与 messages 更新叠成 Maximum update depth exceeded。
   */
  const programmaticScrollRef = useRef(false);

  const syncToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const next = !nearBottom(el) && el.scrollHeight > el.clientHeight;
    setShowToBottom((prev) => (prev === next ? prev : next));
  }, []);

  /** 写 scrollTop 并短时屏蔽 scroll→setState；随后再同步按钮显隐。 */
  const setScrollTopSafe = useCallback(
    (top: number) => {
      const el = scrollRef.current;
      if (!el) return;
      programmaticScrollRef.current = true;
      el.scrollTop = top;
      requestAnimationFrame(() => {
        programmaticScrollRef.current = false;
        syncToBottom();
      });
    },
    [syncToBottom]
  );

  const stick = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTopSafe(el.scrollHeight);
  }, [setScrollTopSafe]);

  // 消息变化时的滚动策略：前插历史 → 差值复位；首次 → 跳底；贴底 → 跟最新。
  // 所有 scrollTop 写入都走 setScrollTopSafe（rAF 外的写 + 屏蔽 onScroll setState）。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || messages.length === 0) return;
    if (restoreRef.current) {
      const { prevHeight, prevTop } = restoreRef.current;
      restoreRef.current = null;
      pinnedRef.current = false;
      requestAnimationFrame(() => {
        const cur = scrollRef.current;
        if (!cur) return;
        setScrollTopSafe(cur.scrollHeight - prevHeight + prevTop);
      });
    } else if (!jumpedRef.current) {
      jumpedRef.current = true;
      pinnedRef.current = true;
      requestAnimationFrame(() => {
        const cur = scrollRef.current;
        if (!cur) return;
        setScrollTopSafe(cur.scrollHeight);
      });
    } else if (pinnedRef.current) {
      requestAnimationFrame(() => {
        const cur = scrollRef.current;
        if (!cur || !pinnedRef.current) return;
        setScrollTopSafe(cur.scrollHeight);
      });
    } else {
      syncToBottom();
    }
  }, [messages, setScrollTopSafe, syncToBottom]);

  // 向上分页：滚近顶部时拉更早的一页（游标 = 已加载最旧的 db id）。
  const loadOlder = useCallback(async () => {
    if (!inTauri || loadingRef.current || !hasMoreRef.current) return;
    const before = oldestIdRef.current;
    if (before === null) return;
    loadingRef.current = true;
    try {
      const rows = await dialogueHistory(currentId, PAGE_SIZE, before);
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
  }, [currentId]);

  function onMessagesScroll() {
    const el = scrollRef.current;
    if (!el) return;
    if (!programmaticScrollRef.current) {
      pinnedRef.current = nearBottom(el);
      syncToBottom();
    }
    if (el.scrollTop <= SCROLL_TOP_TRIGGER) void loadOlder();
  }

  // 进入对话页时聚焦输入框
  useEffect(() => {
    document.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus({
      preventScroll: true,
    });
  }, []);

  // 会话载入（首次挂载 + 每次切换共用一条路）：只拉第一页（30 条），其余
  // 滚到顶部再分页加载。
  const loadedSessionRef = useRef<string | null>(null);
  useEffect(() => {
    const switching =
      loadedSessionRef.current !== null && loadedSessionRef.current !== currentId;
    loadedSessionRef.current = currentId;

    const store = useChatStore.getState();
    if (switching) {
      store.reset();
      oldestIdRef.current = null;
      hasMoreRef.current = true;
      restoreRef.current = null;
      jumpedRef.current = false;
      pinnedRef.current = true;
    } else if (store.messages.length > 0) {
      return;
    }
    if (!inTauri) return;

    let cancelled = false; // 快速连切：只认最后一次请求
    void (async () => {
      try {
        const rows = await dialogueHistory(currentId, PAGE_SIZE);
        if (cancelled || useChatStore.getState().messages.length > 0) return;
        if (rows.length === 0) return;
        oldestIdRef.current = rows[0].id;
        hasMoreRef.current = rows.length === PAGE_SIZE;
        useChatStore.getState().prefill(bubblesFromHistory(rows));
      } catch {
        // 历史读不到就当这个会话是空的，不挡本次对话。
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [currentId]);

  // 会话索引只在壳层拉一次；抽屉每次打开会自己重拉（标题/条数会变）。
  useEffect(() => {
    void useSessionStore.getState().load();
  }, []);

  const empty = messages.length === 0;

  return (
    <>
      <ChatChrome />

      <div className="chat-body">
        <div className="messages" ref={scrollRef} onScroll={onMessagesScroll}>
          {empty && (
            <div className="empty">
              <EmptyStateFace />
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
              <MessageBubble
                key={m.id}
                message={m}
                onStick={stick}
                onRetry={m.kind === "bot" && m.failure?.retryable ? retry : undefined}
              />
            ))}
        </div>
        {showToBottom && (
          <button
            className="to-bottom"
            aria-label="回到最新"
            onClick={() => {
              const el = scrollRef.current;
              if (!el) return;
              programmaticScrollRef.current = true;
              el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
              requestAnimationFrame(() => {
                programmaticScrollRef.current = false;
                syncToBottom();
              });
            }}
          >
            ↓
          </button>
        )}
      </div>

      <Composer onSend={send} onCancel={cancel} chatBusy={chatting} />
      <SessionDrawer open={drawerOpen} onClose={closeDrawer} busy={chatting} />
    </>
  );
}
