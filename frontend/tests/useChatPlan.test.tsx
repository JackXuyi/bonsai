// plan-chat-agent-inline：planned 任务进 kind=plan 气泡；bot 正文仍只有最终回复。
// 用最小 harness 驱动真实 useChat：mock Channel 拿到 onmessage，再模拟后端
// 在流里发 task_start / task_step / task_done。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

const h = vi.hoisted(() => ({
  channel: null as null | { onmessage: ((m: unknown) => void) | null },
}));

vi.mock("@/lib/tauriInvoke", () => ({
  inTauri: true,
  invokeWithTimeout: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((m: unknown) => void) | null = null;
    constructor() {
      h.channel = this as unknown as { onmessage: ((m: unknown) => void) | null };
    }
  },
}));

const chatMock = vi.fn();
const planListMock = vi.fn();
const cancelMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    dialogueChatStream: (text: string, sid: string, ch: unknown) => chatMock(text, sid, ch),
    dialogueCancel: (taskId: string) => cancelMock(taskId),
    planList: (limit?: number) => planListMock(limit),
    facePreview: vi.fn(async () => undefined),
  };
});

import { useChat } from "@/hooks/useChat";
import { useChatStore } from "@/lib/stores/chatStore";
import { usePlanStore } from "@/lib/stores/planStore";

function Harness() {
  const { send, cancel } = useChat();
  return (
    <>
      <button type="button" onClick={() => void send("先查日程再写摘要")}>
        发
      </button>
      <button type="button" onClick={() => void cancel()}>
        停
      </button>
    </>
  );
}

beforeEach(() => {
  h.channel = null;
  chatMock.mockReset();
  planListMock.mockReset();
  cancelMock.mockReset();
  planListMock.mockResolvedValue([]);
  cancelMock.mockResolvedValue(undefined);
  useChatStore.setState({ messages: [] });
  usePlanStore.setState({ current: null, history: [] });
  chatMock.mockImplementation(async (_text: string, _sid: string, ch: { onmessage: ((m: unknown) => void) | null }) => {
    ch.onmessage?.({ task_step: { event: "task_start", id: "t1", goal: "查日程再写摘要", planned: true } });
    ch.onmessage?.({ task_step: { event: "task_step", id: "t1", seq: 0, tool: "calendar_list", status: "ok" } });
    ch.onmessage?.({ task_step: { event: "task_done", id: "t1", status: "done" } });
    ch.onmessage?.({ delta: "好啦，摘要写完了。" });
    // P1：终态走事件，命令本身立即返回（这里 return undefined）
    ch.onmessage?.({ turn_done: { reply: "好啦，摘要写完了。", turn_id: 7 } });
  });
});

describe("useChat × 计划事件", () => {
  it("planned task_start → kind=plan 气泡；bot 正文只有最终回复", async () => {
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));

    await waitFor(() => {
      const msgs = useChatStore.getState().messages;
      expect(msgs.map((m) => m.kind)).toEqual(["user", "plan", "bot"]);
    });
    const msgs = useChatStore.getState().messages;
    expect(msgs[0].text).toBe("先查日程再写摘要");
    expect(msgs[1].planTaskId).toBe("t1");
    expect(msgs[2].text).toBe("好啦，摘要写完了。");
    const botText = msgs
      .filter((m) => m.kind === "bot")
      .map((m) => m.text)
      .join("|");
    expect(botText).toBe("好啦，摘要写完了。");
  });

  it("同一条流里 planStore 与 plan 气泡同时有数据", async () => {
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));

    await waitFor(() => expect(usePlanStore.getState().current).not.toBeNull());
    const cur = usePlanStore.getState().current!;
    expect(cur.id).toBe("t1");
    expect(cur.steps[0].status).toBe("done");
    expect(planListMock).toHaveBeenCalled();
    expect(
      useChatStore.getState().messages.some((m) => m.kind === "plan" && m.planTaskId === "t1"),
    ).toBe(true);
  });

  it("IPC 等超时：已流出的字不撤，失败挂在 failure 行", async () => {
    chatMock.mockImplementation(async (_t: string, _s: string, ch: { onmessage: ((m: unknown) => void) | null }) => {
      ch.onmessage?.({ delta: "黄山这周前半段……" });
      throw new Error("[IPC] dialogue_chat_stream did not respond within 15000ms");
    });
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));

    await waitFor(() => {
      const bot = useChatStore.getState().messages.filter((m) => m.kind === "bot");
      expect(bot).toHaveLength(1);
      expect(bot[0].text).toContain("黄山这周前半段");
      expect(bot[0].failure?.message).toContain("dialogue_chat_stream");
      expect(bot[0].failure?.retryable).toBe(true);
    });
    // 不该出现「没发出去」这种误判文案；正文也不该被错误串盖掉
    const all = useChatStore.getState().messages.map((m) => m.text).join("|");
    expect(all).not.toContain("没发出去");
    expect(all).not.toMatch(/^Error:/);
  });

  it("turn_error 事件：气泡说明失败原因，不静默", async () => {
    chatMock.mockImplementation(async (_t: string, _s: string, ch: { onmessage: ((m: unknown) => void) | null }) => {
      ch.onmessage?.({ task_step: { event: "task_start", id: "t2", goal: "g", planned: true } });
      ch.onmessage?.({
        turn_error: { kind: "transport", message: "model_unreachable", retryable: true },
      });
    });
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));

    await waitFor(() => {
      const bot = useChatStore.getState().messages.filter((m) => m.kind === "bot");
      expect(bot.some((m) => m.failure?.message.includes("model_unreachable") || m.text.includes("model_unreachable"))).toBe(true);
    });
  });

  it("回合进行中再发一条会被忽略（不并发跑两轮）", async () => {
    let release!: () => void;
    chatMock.mockImplementation(async (_t: string, _s: string, ch: { onmessage: ((m: unknown) => void) | null }) => {
      ch.onmessage?.({ task_step: { event: "task_start", id: "t3", goal: "g", planned: true } });
      await new Promise<void>((r) => {
        release = r;
      });
      ch.onmessage?.({ turn_done: { reply: "好了", turn_id: 1 } });
    });
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));
    await waitFor(() => expect(usePlanStore.getState().current?.id).toBe("t3"));

    fireEvent.click(screen.getByText("发"));
    expect(chatMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      release();
    });
  });

  it("点「停」用本轮 task id 叫停后端", async () => {    let release!: () => void;
    chatMock.mockImplementation(async (_t: string, _s: string, ch: { onmessage: ((m: unknown) => void) | null }) => {
      ch.onmessage?.({ task_step: { event: "task_start", id: "t9", goal: "g", planned: true } });
      await new Promise<void>((r) => {
        release = r;
      });
      ch.onmessage?.({ turn_done: { reply: "停了", turn_id: 1 } });
    });
    render(<Harness />);
    fireEvent.click(screen.getByText("发"));
    await waitFor(() => expect(usePlanStore.getState().current?.id).toBe("t9"));

    fireEvent.click(screen.getByText("停"));
    await waitFor(() => expect(cancelMock).toHaveBeenCalledWith("t9"));
    await act(async () => {
      release();
    });
  });
});
