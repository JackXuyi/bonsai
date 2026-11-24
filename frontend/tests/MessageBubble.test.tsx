import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { MessageBubble } from "@/components/chat/MessageBubble";
import type { ChatMessage } from "@/lib/stores/chatStore";

// 气泡内嵌操作条（spec-s8）用到 next/navigation 的 useRouter
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), back: vi.fn() }),
}));

describe("MessageBubble", () => {
  it("user / bot 形态与 proactive 变体", () => {
    const user: ChatMessage = { id: 1, kind: "user", text: "在忙什么？" };
    const { container, rerender } = render(<MessageBubble message={user} />);
    expect(container.querySelector(".bubble.user")).toBeTruthy();
    expect(container.querySelector(".bubble.user")?.textContent).toContain("在忙什么？");

    const bot: ChatMessage = {
      id: 2,
      kind: "bot",
      text: "在看你写代码",
      proactive: true,
      note: "它自己开口的",
    };
    rerender(<MessageBubble message={bot} />);
    const el = container.querySelector(".bubble.bot")!;
    expect(el.className).toContain("proactive");
    expect(el.querySelector(".badge")?.textContent).toBe("它自己开口的");
  });

  it("流式 streaming 渲染 caret；typing 渲染三点", () => {
    const streaming: ChatMessage = { id: 3, kind: "bot", text: "想", streaming: true };
    const { container, rerender } = render(<MessageBubble message={streaming} />);
    expect(container.querySelector(".caret")).toBeTruthy();

    const typing: ChatMessage = { id: 4, kind: "typing", text: "" };
    rerender(<MessageBubble message={typing} />);
    expect(container.querySelector(".bubble.typing")?.querySelectorAll("i")).toHaveLength(3);
  });

  it("失败行展示 hint；retryable 才出重试按钮", () => {
    const onRetry = vi.fn();
    const failed: ChatMessage = {
      id: 10,
      kind: "bot",
      text: "API Key 或额度不对，去设置里看一下",
      failure: { kind: "auth", message: "API Key 或额度不对，去设置里看一下", retryable: false },
    };
    const { container, rerender } = render(
      <MessageBubble message={failed} onRetry={onRetry} />
    );
    expect(container.querySelector(".turn-failure-text")?.textContent).toContain("API Key");
    expect(container.querySelector(".turn-failure-retry")).toBeNull();

    const retryable: ChatMessage = {
      id: 11,
      kind: "bot",
      text: "云端出故障了（不是你的问题）",
      failure: {
        kind: "server",
        message: "云端出故障了（不是你的问题）",
        retryable: true,
      },
    };
    rerender(<MessageBubble message={retryable} onRetry={onRetry} />);
    const btn = container.querySelector(".turn-failure-retry") as HTMLButtonElement;
    expect(btn).toBeTruthy();
    btn.click();
    expect(onRetry).toHaveBeenCalledWith(11);
  });

  it("bot 消息按 markdown 渲染，user 消息保持原文", () => {
    const bot: ChatMessage = {
      id: 5,
      kind: "bot",
      text: "**重点**：两件事\n\n- 第一条\n- 第二条",
    };
    const { container, unmount } = render(<MessageBubble message={bot} />);
    const md = container.querySelector(".bubble.bot .md")!;
    expect(md).toBeTruthy();
    expect(md.querySelector("strong")?.textContent).toBe("重点");
    expect(md.querySelectorAll("li")).toHaveLength(2);
    // 原始星号不应再露出
    expect(md.textContent).not.toContain("**");
    unmount();

    const user: ChatMessage = { id: 6, kind: "user", text: "**照原样显示**" };
    const userView = render(<MessageBubble message={user} />);
    const body = userView.container.querySelector(".bubble.user .body")!;
    expect(body.textContent).toBe("**照原样显示**");
    expect(userView.container.querySelector(".bubble.user .md")).toBeNull();
  });
});
