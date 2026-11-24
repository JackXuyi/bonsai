import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { AbilityCard } from "@/components/tools/AbilityCard";
import { useUiStore } from "@/lib/stores/uiStore";

const resetMock = vi.fn();
const drainMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return { ...orig, abilityReset: () => resetMock() };
});

vi.mock("@/lib/announcements", () => ({
  drainAbilityAnnouncements: () => drainMock(),
}));

vi.mock("@/lib/stores/uiStore", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/stores/uiStore")>();
  return { ...orig, toast: (msg: string) => toastMock(msg) };
});

beforeEach(() => {
  resetMock.mockReset().mockResolvedValue(undefined);
  drainMock.mockReset().mockResolvedValue(undefined);
  toastMock.mockReset();
  useUiStore.setState({
    ability: {
      stage: 0,
      stage_name: "陪聊",
      day_index: 5,
      pending: [],
    },
  });
});

describe("AbilityCard", () => {
  it("展示阶段文案与天数，无分数", () => {
    render(<AbilityCard />);
    expect(screen.getByText("它办事的本事")).toBeTruthy();
    expect(screen.getByText(/陪聊 · 第 6 天/)).toBeTruthy();
    expect(screen.getByText(/还要 26 天/)).toBeTruthy();
    expect(screen.queryByText(/分/)).toBeNull();
  });

  it("重新练手两步确认后调用 reset", async () => {
    render(<AbilityCard />);
    const btn = screen.getByRole("button", { name: "重新练手" });
    fireEvent.click(btn);
    expect(resetMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "再点一次" }));
    await waitFor(() => expect(resetMock).toHaveBeenCalledTimes(1));
    expect(drainMock).toHaveBeenCalled();
    expect(toastMock).toHaveBeenCalled();
  });
});
