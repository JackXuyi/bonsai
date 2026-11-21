import { beforeEach, describe, expect, it } from "vitest";
import { useChatStore, type ChatMessage } from "@/lib/stores/chatStore";

function reset() {
  useChatStore.setState({ messages: [] });
}

beforeEach(reset);

describe("chatStore", () => {
  it("add 返回自增 id 且按序追加（pushMessage 语义）", () => {
    const id1 = useChatStore.getState().add({ kind: "user", text: "你好" });
    const id2 = useChatStore.getState().add({ kind: "bot", text: "嗯", proactive: true });

    expect(id2).toBe(id1 + 1);
    const msgs = useChatStore.getState().messages;
    expect(msgs.map((m) => m.text)).toEqual(["你好", "嗯"]);
    expect(msgs[1].proactive).toBe(true);
  });

  it("appendText 流式渐增 / setText 整段替换 / setNote 徽注（replaceTyping 语义：typing 占位 remove 后正文气泡渐增）", () => {
    const store = useChatStore.getState();
    const typingId = store.add({ kind: "typing", text: "" });
    store.remove(typingId);
    const bubbleId = store.add({ kind: "bot", text: "", streaming: true });

    useChatStore.getState().appendText(bubbleId, "今天");
    useChatStore.getState().appendText(bubbleId, "天气不错");
    let msg = useChatStore.getState().messages[0];
    expect(msg.text).toBe("今天天气不错");
    expect(msg.streaming).toBe(true);

    useChatStore.getState().setText(bubbleId, "今天天气不错。");
    useChatStore.getState().setNote(bubbleId, "日程 · 直接查的日历");
    msg = useChatStore.getState().messages[0] as ChatMessage;
    expect(msg.text).toBe("今天天气不错。");
    expect(msg.note).toBe("日程 · 直接查的日历");
    // spec-s8 回归：setText 落定正文时必须清掉 streaming，
    // 否则 MessageBubble 的 `!message.streaming` 门永远挡住操作条。
    expect(msg.streaming).toBe(false);

    // 不存在的 id 静默跳过
    expect(() => useChatStore.getState().appendText(999, "x")).not.toThrow();
  });

  it("消息存于模块级 store，跨「页面切换」（setState 重置外）保留", () => {
    // 模拟切走再切回：组件卸载不影响 store；其它 setState（如 uiStore）
    // 也不触碰 messages —— 这里验证 store 引用稳定性 + 内容保留。
    useChatStore.getState().add({ kind: "user", text: "在忙什么？" });
    const before = useChatStore.getState().messages;

    const sameStore = useChatStore; // App Router 下两个路由页拿到同一实例
    expect(sameStore.getState().messages).toBe(before);
    expect(sameStore.getState().messages).toHaveLength(1);
  });
});
