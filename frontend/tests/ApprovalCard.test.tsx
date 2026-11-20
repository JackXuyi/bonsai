import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { ApprovalCard } from "@/components/agent/ApprovalCard";

const replyMock = vi.fn();
// useTauriEvent mock: capture the handler so the test can "emit" an event.
let emit: ((p: unknown) => void) | null = null;

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return { ...orig, mcpApprovalReply: (id: number, allow: boolean) => replyMock(id, allow) };
});

vi.mock("@/lib/tauriListen", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/tauriListen")>();
  return {
    ...orig,
    useTauriEvent: <T,>(_event: string, handler: (p: T) => void) => {
      emit = handler as (p: unknown) => void;
    },
  };
});

function request(over: Record<string, unknown> = {}) {
  return {
    id: 7,
    tool: "weather",
    source: "本地天气",
    remote: true,
    argsSummary: '{"city":"杭州"}',
    ...over,
  };
}

beforeEach(() => {
  replyMock.mockReset();
  replyMock.mockResolvedValue(true);
  emit = null;
});

describe("ApprovalCard", () => {
  it("无事件时不渲染（auto 模式零开销）", () => {
    render(<ApprovalCard />);
    expect(screen.queryByTestId("approval-card")).toBeNull();
  });

  it("事件到达 → 弹卡并标注远程/参数摘要", async () => {
    render(<ApprovalCard />);
    act(() => emit?.(request()));
    expect(await screen.findByTestId("approval-card")).toBeTruthy();
    const text = screen.getByTestId("approval-card").textContent ?? "";
    expect(text).toContain("weather");
    expect(text).toContain("参数会出设备");
    expect(text).toContain("30 秒内不回应视为拒绝");
    expect(text).toContain("杭州");
  });

  it("允许 → 回执 id+allow=true，卡片关闭", async () => {
    render(<ApprovalCard />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("允许本次"));
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, true));
    await waitFor(() => expect(screen.queryByTestId("approval-card")).toBeNull());
  });

  it("拒绝 → 回执 allow=false", async () => {
    render(<ApprovalCard />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("拒绝"));
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, false));
  });

  it("回执返回 false（已超时/已回答）→ 卡片留显过期提示", async () => {
    replyMock.mockResolvedValue(false);
    render(<ApprovalCard />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("允许本次"));
    expect(await screen.findByText(/这次请求已经过期/)).toBeTruthy();
    // 过期态只剩「知道了」，不再能重复回执
    expect(screen.queryByText("允许本次")).toBeNull();
    fireEvent.click(screen.getByText("知道了"));
    await waitFor(() => expect(screen.queryByTestId("approval-card")).toBeNull());
  });
});
