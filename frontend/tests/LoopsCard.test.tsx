import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { LoopsCard } from "@/components/settings/LoopsCard";

const statusMock = vi.fn();
const getAutoMock = vi.fn();
const setAutoMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    loopsStatus: (...args: unknown[]) => statusMock(...args),
    loopAutorestartGet: (...args: unknown[]) => getAutoMock(...args),
    loopAutorestartSet: (...args: unknown[]) => setAutoMock(...args),
  };
});

vi.mock("@/lib/tauriListen", () => ({
  useTauriEvent: () => {},
}));

beforeEach(() => {
  statusMock.mockReset();
  getAutoMock.mockReset();
  setAutoMock.mockReset();
  statusMock.mockResolvedValue([
    {
      name: "face",
      state: "running",
      last_beat: new Date().toISOString(),
      stopped_at: null,
      detail: null,
      restarts: 0,
    },
    {
      name: "calendar",
      state: "stopped",
      last_beat: null,
      stopped_at: new Date().toISOString(),
      detail: "consecutive panic cap",
      restarts: 1,
    },
  ]);
  getAutoMock.mockResolvedValue(false);
  setAutoMock.mockResolvedValue(true);
});

describe("LoopsCard", () => {
  it("渲染八行口径的状态列表（含 running / stopped）", async () => {
    render(<LoopsCard />);
    expect(await screen.findByTestId("loops-list")).toBeTruthy();
    expect(screen.getByText("表情态机")).toBeTruthy();
    expect(screen.getByText("日历提醒")).toBeTruthy();
    expect(screen.getByText(/已停 · consecutive panic cap/)).toBeTruthy();
    expect(screen.getByText(/已拉起 1/)).toBeTruthy();
  });

  it("拨动自动拉起开关会调用 set 命令", async () => {
    render(<LoopsCard />);
    await screen.findByTestId("loops-list");
    const toggle = screen.getByRole("checkbox");
    expect((toggle as HTMLInputElement).checked).toBe(false);
    fireEvent.click(toggle);
    await waitFor(() => {
      expect(setAutoMock).toHaveBeenCalledWith(true);
    });
  });
});
