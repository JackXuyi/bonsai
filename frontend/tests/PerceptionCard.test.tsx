import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { PerceptionCard } from "@/components/tools/PerceptionCard";
import type { PerceptionConfig } from "@/lib/types";

const getCfg = vi.fn();
const setCfg = vi.fn();
const statusMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    perceptionGetConfig: () => getCfg(),
    perceptionSetConfig: (cfg: PerceptionConfig) => setCfg(cfg),
    perceptionStatus: () => statusMock(),
  };
});

vi.mock("@/lib/stores/uiStore", () => ({
  toast: (...args: unknown[]) => toastMock(...args),
  useUiStore: (sel: (s: { setToolResult: () => void; windowShownAt: number }) => unknown) =>
    sel({ setToolResult: () => {}, windowShownAt: 0 }),
}));

function fixture(over: Partial<PerceptionConfig> = {}): PerceptionConfig {
  return {
    enabled: true,
    clipboard: false,
    active_app: true,
    downloads: false,
    window_title: false,
    title_allowlist: [],
    taught_nouns: [],
    ...over,
  };
}

beforeEach(() => {
  for (const m of [getCfg, setCfg, statusMock, toastMock]) m.mockReset();
  getCfg.mockResolvedValue(fixture());
  setCfg.mockImplementation(async (cfg: PerceptionConfig) => cfg);
  statusMock.mockResolvedValue({
    enabled: true,
    idle_secs: 12,
    front_app: "Cursor",
    front_title: null,
  });
});

describe("PerceptionCard", () => {
  it("开启窗口标题后展示白名单与教名词", async () => {
    getCfg.mockResolvedValue(fixture({ window_title: true }));
    render(<PerceptionCard active />);
    await waitFor(() => expect(getCfg).toHaveBeenCalled());
    const card = screen.getByTestId("perception-card");
    await waitFor(() =>
      expect(card.textContent).toContain("仅勾选的 App 会记窗口粗标签")
    );
    expect(card.textContent).toContain("教名词");
    fireEvent.click(screen.getByText(/加入当前前台/));
    await waitFor(() =>
      expect(setCfg).toHaveBeenCalledWith(
        expect.objectContaining({ title_allowlist: ["Cursor"] })
      )
    );
  });
});
