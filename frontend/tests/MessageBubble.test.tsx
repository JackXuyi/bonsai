import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { MessageBubble } from "@/components/chat/MessageBubble";
import type { ChatMessage } from "@/lib/stores/chatStore";

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
});
