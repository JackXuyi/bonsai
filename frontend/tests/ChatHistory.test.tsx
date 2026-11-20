"use client";

// 对话页启动回放：挂载时拉 dialogue_history 灌入消息流（只在流为空时），
// proactive 标注、跨天日期徽标、失败静默降级。
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { ChatPane } from "@/components/chat/ChatPane";
import { useChatStore } from "@/lib/stores/chatStore";

// inTauri 在 tauriInvoke 模块求值时固定，测试用 getter 开关模拟 Tauri 环境。
const h = vi.hoisted(() => ({ tauri: true }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), back: vi.fn() }),
}));

vi.mock("@/lib/tauriInvoke", () => ({
  get inTauri() {
    return h.tauri;
  },
  invokeWithTimeout: vi.fn().mockRejectedValue(new Error("ipc not under test")),
}));

const historyMock = vi.fn().mockResolvedValue([]);

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    dialogueHistory: (...args: unknown[]) => historyMock(...args),
    // 发送路径不在本测试范围；给个安全的 no-op。
    dialogueChatStream: vi.fn().mockRejectedValue(new Error("not under test")),
  };
});

const yesterday = new Date(Date.now() - 24 * 3600 * 1000);
const ts = (d: Date) => d.toISOString();

beforeEach(() => {
  h.tauri = true;
  useChatStore.setState({ messages: [] });
  historyMock.mockReset().mockResolvedValue([]);
});

describe("ChatPane 启动回放", () => {
  it("把历史行灌成气泡：user/bot 分条、无前导 user 的 assistant 标 proactive", async () => {
    historyMock.mockResolvedValue([
      { id: 1, ts: ts(yesterday), role: "user", content: "早上好" },
      { id: 2, ts: ts(yesterday), role: "assistant", content: "早。今天有雨。" },
      { id: 3, ts: ts(yesterday), role: "assistant", content: "（盆栽主动说的内容）" },
      { id: 4, ts: ts(new Date()), role: "user", content: "今天聊的" },
    ]);

    render(<ChatPane />);

    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(4));
    const msgs = useChatStore.getState().messages;
    expect(msgs[0].kind).toBe("user");
    expect(msgs[1].kind).toBe("bot");
    expect(msgs[1].proactive).toBeFalsy();
    expect(msgs[2].proactive).toBe(true); // 无前导 user → 主动搭话
    expect(msgs[3].proactive).toBeFalsy();
    // 跨天日期徽标：昨天的带日期，今天的不带。
    const d = yesterday;
    expect(msgs[0].note).toBe(`${d.getMonth() + 1}-${d.getDate()}`);
    expect(msgs[3].note).toBeUndefined();
    // 空态消失，正文可见。
    expect(screen.queryByText("还没聊过")).toBeNull();
    expect(screen.getByText("早。今天有雨。")).toBeTruthy();
  });

  it("消息流已有内容（本次启动先聊过）时不覆盖", async () => {
    useChatStore.getState().add({ kind: "user", text: "刚发的" });
    historyMock.mockResolvedValue([
      { id: 1, ts: ts(yesterday), role: "user", content: "昨天的旧消息" },
    ]);

    render(<ChatPane />);

    await waitFor(() => expect(historyMock).toHaveBeenCalled());
    await waitFor(() =>
      expect(useChatStore.getState().messages.map((m) => m.text)).toEqual(["刚发的"])
    );
    expect(screen.queryByText("昨天的旧消息")).toBeNull();
  });

  it("拉取失败静默降级：保持空态", async () => {
    historyMock.mockRejectedValue(new Error("db locked"));

    render(<ChatPane />);

    await waitFor(() => expect(historyMock).toHaveBeenCalled());
    // 给 catch 分支足够的时间跑完，然后确认没有灌入任何消息。
    await new Promise((r) => setTimeout(r, 50));
    expect(useChatStore.getState().messages).toHaveLength(0);
    expect(screen.getByText("还没聊过")).toBeTruthy();
  });

  it("浏览器预览（非 Tauri）不拉历史", async () => {
    h.tauri = false;

    render(<ChatPane />);
    await new Promise((r) => setTimeout(r, 20));
    expect(historyMock).not.toHaveBeenCalled();
    expect(screen.getByText("还没聊过")).toBeTruthy();
  });
});

describe("ChatPane 向上分页", () => {
  const PAGE = 30;
  const mkRows = (from: number, count: number) =>
    // 后端契约：旧→新（ASC）。from = 本页最新一行的 id。
    Array.from({ length: count }, (_, i) => ({
      id: from - count + 1 + i,
      ts: ts(yesterday),
      role: (i % 2 === 0 ? "user" : "assistant") as "user" | "assistant",
      content: `row-${from - count + 1 + i}`,
    }));

  it("初次只拉一页；滚到顶部再拉更早一页并前插到最前面", async () => {
    const page1 = mkRows(100, PAGE); // id 100..71
    const page2 = mkRows(70, 5); // id 70..66
    historyMock.mockResolvedValueOnce(page1).mockResolvedValueOnce(page2);

    const { container } = render(<ChatPane />);
    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(PAGE));

    // 初次调用不带游标；游标 = 第一页最旧行的 id
    expect(historyMock).toHaveBeenCalledTimes(1);
    expect(historyMock.mock.calls[0][0]).toBe("default");
    expect(historyMock.mock.calls[0][1]).toBe(PAGE);
    expect(historyMock.mock.calls[0][2]).toBeUndefined();

    // 模拟滚到顶部
    const list = container.querySelector(".messages")!;
    Object.defineProperty(list, "scrollTop", { value: 0, writable: true });
    fireEvent.scroll(list);

    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(PAGE + 5));
    const msgs = useChatStore.getState().messages;
    expect(msgs[0].text).toBe("row-66"); // 第二页最旧行在最前
    expect(msgs[4].text).toBe("row-70"); // 第二页最新行
    expect(msgs[5].text).toBe("row-71"); // 第一页最旧行紧随第二页
    expect(msgs[PAGE + 4].text).toBe("row-100"); // 第一页最新行
    // 第二次调用带上了游标（第一页最旧行的 id）
    expect(historyMock).toHaveBeenLastCalledWith("default", PAGE, 71);
  });

  it("返回不足一页 → 到底，之后滚动不再请求", async () => {
    historyMock
      .mockResolvedValueOnce(mkRows(100, PAGE))
      .mockResolvedValueOnce(mkRows(70, 3)); // 不足一页

    const { container } = render(<ChatPane />);
    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(PAGE));

    const list = container.querySelector(".messages")!;
    Object.defineProperty(list, "scrollTop", { value: 0, writable: true });
    fireEvent.scroll(list);
    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(PAGE + 3));

    const calls = historyMock.mock.calls.length;
    fireEvent.scroll(list);
    await new Promise((r) => setTimeout(r, 30));
    expect(historyMock.mock.calls.length).toBe(calls); // hasMore=false，不再拉
  });

  it("页尺寸恢复为 30（避免一次灌太多导致卡顿）", async () => {
    historyMock.mockResolvedValue(mkRows(100, PAGE));
    render(<ChatPane />);
    await waitFor(() => expect(useChatStore.getState().messages).toHaveLength(PAGE));
    expect(historyMock.mock.calls[0][1]).toBe(30);
  });
});
