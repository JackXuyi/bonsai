import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { ObservabilityCard } from "@/components/settings/ObservabilityCard";

const exportMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    observabilityDayExport: (...args: unknown[]) => exportMock(...args),
  };
});

vi.mock("@/lib/stores/uiStore", () => ({
  toast: (...args: unknown[]) => toastMock(...args),
}));

beforeEach(() => {
  exportMock.mockReset();
  toastMock.mockReset();
  exportMock.mockResolvedValue({
    day: "2026-11-24",
    markdown: "# bonsai 日时间线 · 2026-11-24\n\n## 安全 / 注入留痕\n\n_（无）_\n",
    turns: 0,
    tools: 0,
    security: 0,
  });
  Object.assign(navigator, {
    clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});

describe("ObservabilityCard", () => {
  it("导出今日并展示预览", async () => {
    render(<ObservabilityCard />);
    fireEvent.click(screen.getByText("复制今日 Markdown"));
    await waitFor(() => expect(exportMock).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByTestId("obs-preview").textContent).toContain("日时间线")
    );
  });
});
