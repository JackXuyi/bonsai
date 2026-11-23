"use client";

// 复现「Maximum update depth exceeded」：真实 app 里流式 delta 高频到达 +
// 滚动贴底。这里模拟滚动尺寸（jsdom 没有布局）+ 200 个连续 delta + 收尾
// 四连发（setText/setNote/patch×2），React 的嵌套更新检测会直接报错。
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render } from "@testing-library/react";
import { ChatPane } from "@/components/chat/ChatPane";
import { useChatStore } from "@/lib/stores/chatStore";

const h = vi.hoisted(() => ({ tauri: false }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), back: vi.fn() }),
}));

vi.mock("@/lib/tauriInvoke", () => ({
  get inTauri() {
    return h.tauri;
  },
  invokeWithTimeout: vi.fn().mockRejectedValue(new Error("ipc not under test")),
}));

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    dialogueHistory: vi.fn().mockRejectedValue(new Error("not under test")),
    dialogueChatStream: vi.fn().mockRejectedValue(new Error("not under test")),
  };
});

import { dialogueHistory } from "@/lib/commands";

beforeEach(() => {
  h.tauri = false;
  useChatStore.setState({ messages: [] });
});

/** 给 .messages 元素注入可增长的滚动尺寸（近似真实布局）。 */
function mockMetrics(container: HTMLElement) {
  const el = container.querySelector(".messages") as HTMLElement;
  const m = { height: 400, top: 0 };
  Object.defineProperty(el, "clientHeight", { get: () => 400, configurable: true });
  Object.defineProperty(el, "scrollHeight", {
    get: () => m.height,
    set: (v: number) => {
      m.height = Math.max(m.height, v);
    },
    configurable: true,
  });
  Object.defineProperty(el, "scrollTop", {
    get: () => m.top,
    set: (v: number) => {
      m.top = v;
      el.dispatchEvent(new Event("scroll"));
    },
    configurable: true,
  });
  return { el, m };
}

describe("流式压力：无嵌套更新循环", () => {
  it("200 个连续 delta + 收尾四连发不触发 Maximum update depth", async () => {
    const { container } = render(<ChatPane />);
    const { el, m } = mockMetrics(container);
    const store = useChatStore.getState();

    act(() => {
      store.add({ kind: "user", text: "在忙什么？" });
    });

    // 首个 delta：撤 typing、建流式气泡
    let bubbleId: number | null = null;
    const chunks = Array.from({ length: 200 }, (_, i) => `第${i}段**内容**。`);

    for (const c of chunks) {
      act(() => {
        if (bubbleId === null) {
          bubbleId = store.add({ kind: "bot", text: "", streaming: true });
        }
        store.appendText(bubbleId, c);
        el.scrollTop = el.scrollHeight; // 贴底（触发 scroll 事件）
      });
    }

    // 收尾四连发（与 useChat.send 完成路径一致）
    act(() => {
      store.setText(bubbleId!, chunks.join(""));
      store.setNote(bubbleId!, "工具调用 · memory_search");
      store.patch(bubbleId!, { turnId: 42 });
      store.patch(bubbleId!, { eventId: 43 });
    });

    expect(useChatStore.getState().messages).toHaveLength(2);
    expect(useChatStore.getState().messages[1].streaming).toBeFalsy();
  });

  it("历史回放 + 上翻分页的恢复滚动不循环", async () => {
    h.tauri = true;
    const history = dialogueHistory as unknown as ReturnType<typeof vi.fn>;
    let remaining = 6; // 模拟 6 页后到底
    history.mockImplementation(async (_s: string, n: number, before?: number) => {
      if (remaining-- <= 0) return [];
      const base = before ?? 10_000;
      return Array.from({ length: n }, (_, i) => ({
        id: base - i,
        ts: new Date().toISOString(),
        role: i % 2 === 0 ? ("user" as const) : ("assistant" as const),
        content: `row-${base - i} **重点**内容`,
      }));
    });

    const { container } = render(<ChatPane />);
    const { el, m } = mockMetrics(container);

    // 等首轮 prefill
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });

    // 连续向上滚 5 次（每次都触发 loadOlder → prepend → restore）
    for (let i = 0; i < 5; i++) {
      m.top = 0; // 滚到顶
      await act(async () => {
        el.dispatchEvent(new Event("scroll"));
        await new Promise((r) => setTimeout(r, 30));
      });
    }
    expect(useChatStore.getState().messages.length).toBeGreaterThan(30);
  });
});
