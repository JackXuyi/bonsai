import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { DiagnosticsCard } from "@/components/settings/DiagnosticsCard";

const recentMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    failuresRecent: (...args: unknown[]) => recentMock(...args),
  };
});

beforeEach(() => {
  recentMock.mockReset();
  recentMock.mockResolvedValue([
    {
      source: "turn",
      kind: "timeout",
      hint: "它想太久了，没等到结果",
      at: "2026-11-24T12:00:00+08:00",
    },
    {
      source: "consolidate",
      kind: "transport",
      hint: "网络没连上，等一下再试",
      at: "2026-11-24T11:30:00+08:00",
    },
  ]);
});

describe("DiagnosticsCard", () => {
  it("渲染最近失败行（kind / hint / 来源）", async () => {
    render(<DiagnosticsCard />);
    expect(await screen.findByTestId("diagnostics-list")).toBeTruthy();
    expect(screen.getByText("对话")).toBeTruthy();
    expect(screen.getByText("夜间整理")).toBeTruthy();
    expect(screen.getByText("它想太久了，没等到结果")).toBeTruthy();
    expect(screen.getByText(/timeout/)).toBeTruthy();
  });

  it("空列表显示占位文案", async () => {
    recentMock.mockResolvedValue([]);
    render(<DiagnosticsCard />);
    expect(await screen.findByTestId("diagnostics-empty")).toBeTruthy();
    expect(screen.getByText("最近没有记到失败")).toBeTruthy();
  });

  it("点刷新会再拉一次", async () => {
    render(<DiagnosticsCard />);
    await screen.findByTestId("diagnostics-list");
    expect(recentMock).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() => {
      expect(recentMock).toHaveBeenCalledTimes(2);
    });
  });
});
