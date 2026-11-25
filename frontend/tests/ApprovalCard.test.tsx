import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { InlineApprovalBar } from "@/components/agent/InlineApprovalBar";

const replyMock = vi.fn();
let emit: ((p: unknown) => void) | null = null;

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    mcpApprovalReply: (id: number, allow: boolean, scope?: string) =>
      replyMock(id, allow, scope ?? "once"),
  };
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

describe("InlineApprovalBar", () => {
  it("无事件时不渲染（auto 模式零开销）", () => {
    render(<InlineApprovalBar />);
    expect(screen.queryByTestId("approval-card")).toBeNull();
  });

  it("onActiveChange：出现/关闭待批时通知（供 Composer 藏 ChatPet）", async () => {
    const onActive = vi.fn();
    const { unmount } = render(<InlineApprovalBar onActiveChange={onActive} />);
    expect(onActive).toHaveBeenCalledWith(false);
    act(() => emit?.(request()));
    await screen.findByTestId("approval-card");
    expect(onActive).toHaveBeenCalledWith(true);
    fireEvent.click(screen.getByText("允许本次"));
    await waitFor(() => expect(screen.queryByTestId("approval-card")).toBeNull());
    expect(onActive).toHaveBeenCalledWith(false);
    unmount();
    expect(onActive).toHaveBeenLastCalledWith(false);
  });

  it("事件到达 → 非模态条并标注远程/参数摘要", async () => {
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    const card = await screen.findByTestId("approval-card");
    expect(card).toBeTruthy();
    expect(card.getAttribute("aria-modal")).toBeNull();
    const text = card.textContent ?? "";
    expect(text).toContain("weather");
    expect(text).toContain("参数会出设备");
    expect(text).toMatch(/秒内不回应视为拒绝/);
    expect(text).toContain("杭州");
  });

  it("允许 → 回执 id+allow=true+once，卡片关闭", async () => {
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("允许本次"));
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, true, "once"));
    await waitFor(() => expect(screen.queryByTestId("approval-card")).toBeNull());
  });

  it("本会话允许 → scope=session", async () => {
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("本会话允许"));
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, true, "session"));
  });

  it("拒绝 → 回执 allow=false", async () => {
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("拒绝"));
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, false, "once"));
  });

  it("Esc → 拒绝", async () => {
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    await screen.findByText("允许本次");
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await waitFor(() => expect(replyMock).toHaveBeenCalledWith(7, false, "once"));
  });

  it("参数摘要按键值行展示（可读）", async () => {
    render(<InlineApprovalBar />);
    act(() =>
      emit?.(
        request({
          tool: "fs_write",
          argsSummary: "路径：/tmp/a.md\n内容预览：hello",
        }),
      ),
    );
    const args = await screen.findByTestId("tool-args");
    expect(args.textContent).toContain("路径");
    expect(args.textContent).toContain("/tmp/a.md");
    expect(args.textContent).toContain("内容预览");
  });

  it("回执返回 false（已超时/已回答）→ 卡片留显过期提示", async () => {
    replyMock.mockResolvedValue(false);
    render(<InlineApprovalBar />);
    act(() => emit?.(request()));
    fireEvent.click(await screen.findByText("允许本次"));
    expect(await screen.findByText(/这次请求已经过期/)).toBeTruthy();
    expect(screen.queryByText("允许本次")).toBeNull();
    fireEvent.click(screen.getByText("知道了"));
    await waitFor(() => expect(screen.queryByTestId("approval-card")).toBeNull());
  });
});
