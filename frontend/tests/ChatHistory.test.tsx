"use client";

// 对话页启动回放：挂载时拉 dialogue_history 灌入消息流（只在流为空时），
// proactive 标注、跨天日期徽标、失败静默降级。
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ChatPane } from "@/components/chat/ChatPane";
import { useChatStore } from "@/lib/stores/chatStore";

// inTauri 在 tauriInvoke 模块求值时固定，测试用 getter 开关模拟 Tauri 环境。
const h = vi.hoisted(() => ({ tauri: true }));

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
  historyMock.mockClear().mockResolvedValue([]);
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
