"use client";

// 对话页（旧 tab-chat）：消息流 + 渐增气泡 + typing + 回到底部 + composer。
// 主动搭话 / 预测 / 提醒事件已上移 useAppEvents（路由化后 ChatPane 只在 /
// 挂载）；消息流状态在 chatStore，跨页保留。
//
// 多会话（plan-context.md §6.8）：顶部一颗「会话」药丸开右侧抽屉。切换时
// 消息流整条清空再按新会话回放（A12 不串味）；一轮在飞时抽屉禁点（A13）。
import { useCallback, useEffect, useRef, useState } from "react";
import { MessageBubble } from "./MessageBubble";
import { Composer } from "./Composer";
import { SessionDrawer } from "./SessionDrawer";
import { useChat } from "@/hooks/useChat";
import { useChatStore, type ChatMessage } from "@/lib/stores/chatStore";
import {
  sessionTitle,
  useCurrentSession,
  useSessionStore,
} from "@/lib/stores/sessionStore";
import { dialogueHistory } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";

function nearBottom(el: HTMLElement) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 56;
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
  const current = useCurrentSession();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [showToBottom, setShowToBottom] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);

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
  /** 用户是否贴底：贴底时流式增量涨多少都跟着滚（2026-11-23 用户裁决：
   *  流式内容要自动出现在可见区，不能因为一次增量超过 56px 就脱底）；
   *  用户往上翻就看别打扰。onScroll 里维护，渲染 effect 里消费。 */
  const pinnedRef = useRef(true);

  const stick = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    syncToBottom();
  }, []);

  function syncToBottom() {
    const el = scrollRef.current;
    if (!el) return;
    setShowToBottom(!nearBottom(el) && el.scrollHeight > el.clientHeight);
  }

  // 消息变化时的滚动策略：前插历史页 → 按差值复位（用户在顶部翻旧消息，
  // 不贴底）；首次有内容 → 跳到最新；贴底（含流式增量）→ 始终跟到最新；
  // 不贴底 → 只同步「回到底部」按钮显隐，不打扰用户阅读。
  // 贴底写入放进 rAF：DOM 滚动写在 passive effect 的 flush 阶段内会与
  // scroll 事件回调交错（WebKit 真机可同步派发），onScroll 里的 setState
  // 就可能滚进嵌套更新链 → Maximum update depth exceeded。rAF 把写入挪到
  // flush 之外，事件回调变成普通任务，链路必断。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || messages.length === 0) return;
    if (restoreRef.current) {
      const { prevHeight, prevTop } = restoreRef.current;
      restoreRef.current = null;
      pinnedRef.current = false;
      el.scrollTop = el.scrollHeight - prevHeight + prevTop;
      syncToBottom();
    } else if (!jumpedRef.current) {
      jumpedRef.current = true;
      pinnedRef.current = true;
      el.scrollTop = el.scrollHeight;
      syncToBottom();
    } else if (pinnedRef.current) {
      requestAnimationFrame(() => {
        const cur = scrollRef.current;
        if (!cur || !pinnedRef.current) return;
        cur.scrollTop = cur.scrollHeight;
        syncToBottom();
      });
    } else {
      syncToBottom();
    }
  }, [messages]);

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
    if (el) pinnedRef.current = nearBottom(el);
    syncToBottom();
    if (el && el.scrollTop <= SCROLL_TOP_TRIGGER) void loadOlder();
  }

  // 进入对话页时聚焦输入框（旧 selectTab("chat")）
  useEffect(() => {
    scrollRef.current?.querySelector("textarea")?.focus({ preventScroll: true });
  }, []);

  // 会话载入（首次挂载 + 每次切换共用一条路）：只拉第一页（30 条），其余
  // 滚到顶部再分页加载。
  // - 切换：先把旧会话的气泡整条清掉、游标归零（A12 旧气泡不残留），
  //   再按新会话回放；`jumpedRef` 复位让视图落到底部。
  // - 首次挂载：本次启动若已聊过（先发消息 / proactive 先到）就以消息流
  //   为准，不覆盖；否则按 DB 回放。跨启动一律以 DB 为准，不做本地缓存。
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
      <div className="chat-topbar">
        <button
          type="button"
          className={`sess-pill ${chatting ? "busy" : ""}`}
          onClick={() => setDrawerOpen(true)}
          title={chatting ? "这一轮说完就能换会话" : "会话"}
          aria-label="会话"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="4" y1="7" x2="20" y2="7" />
            <line x1="4" y1="12" x2="13" y2="12" />
            <line x1="4" y1="17" x2="16" y2="17" />
          </svg>
          <span className="sess-pill-title">{sessionTitle(current)}</span>
        </button>
      </div>

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
              el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
            }}
          >
            ↓
          </button>
        )}
      </div>

      <Composer onSend={send} onCancel={cancel} chatBusy={chatting} />
      <SessionDrawer open={drawerOpen} onClose={() => setDrawerOpen(false)} busy={chatting} />
    </>
  );
}
