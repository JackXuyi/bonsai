import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { MessageActions } from "@/components/chat/MessageActions";
import type { ChatMessage } from "@/lib/stores/chatStore";

// router mock：next/navigation 换成可断言的 push
const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, back: vi.fn() }),
}));

// inTauri 在模块求值时固定，用 getter 开关（沿 ChatHistory.test 惯例）
const h = vi.hoisted(() => ({ tauri: true }));
vi.mock("@/lib/tauriInvoke", () => ({
  get inTauri() {
    return h.tauri;
  },
  invokeWithTimeout: vi.fn(),
}));

// commands mock：只用到反查
const findMock = vi.fn();
vi.mock("@/lib/commands", () => ({
  turnTraceFindByEvent: (...args: unknown[]) => findMock(...args),
}));

// clipboard mock
const writeText = vi.fn().mockResolvedValue(undefined);
Object.defineProperty(navigator, "clipboard", {
  value: { writeText },
  configurable: true,
});

function msg(partial: Partial<ChatMessage>): ChatMessage {
  return { id: 1, kind: "bot", text: "明天上午有两条安排", ts: "2026-11-20T14:32:00+08:00", ...partial };
}

describe("MessageActions（spec-s8 气泡操作条）", () => {
  beforeEach(() => {
    h.tauri = true;
    push.mockClear();
    writeText.mockClear();
    findMock.mockReset();
  });

  it("时间常驻；复制写入剪贴板；无 turnId/eventId 时不渲染详情入口", () => {
    render(<MessageActions message={msg({})} />);
    expect(screen.getByText("复制")).toBeTruthy();
    expect(screen.getByText(/14:32/)).toBeTruthy();
    expect(screen.queryByText(/查看详情/)).toBeNull();

    fireEvent.click(screen.getByText("复制"));
    expect(writeText).toHaveBeenCalledWith("明天上午有两条安排");
  });

  it("带 turnId 直接跳 /trace/[turnId]", () => {
    render(<MessageActions message={msg({ turnId: 412 })} />);
    fireEvent.click(screen.getByText(/查看详情/));
    expect(push).toHaveBeenCalledWith("/trace/412");
  });

  it("只有 eventId 时反查；查不到不跳转（toast 由 uiStore 承接）", async () => {
    findMock.mockResolvedValue(null);
    render(<MessageActions message={msg({ eventId: 9001 })} />);
    fireEvent.click(screen.getByText(/查看详情/));
    await vi.waitFor(() => expect(findMock).toHaveBeenCalledWith(9001));
    expect(push).not.toHaveBeenCalled();
  });

  it("反查命中 turnId 后跳转", async () => {
    findMock.mockResolvedValue(77);
    render(<MessageActions message={msg({ eventId: 9002, kind: "user" })} />);
    fireEvent.click(screen.getByText(/查看详情/));
    await vi.waitFor(() => expect(push).toHaveBeenCalledWith("/trace/77"));
  });

  it("非 Tauri 环境不反查", async () => {
    h.tauri = false;
    render(<MessageActions message={msg({ eventId: 9003 })} />);
    fireEvent.click(screen.getByText(/查看详情/));
    expect(findMock).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
  });
});
