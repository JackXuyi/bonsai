import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { ModelSection } from "@/components/settings/ModelSection";
import { useLlmStore } from "@/lib/stores/llmStore";

vi.mock("@/hooks/useModels", () => ({
  useModels: () => {},
}));

vi.mock("@/components/settings/CloudSection", () => ({
  CloudSection: () => <div>cloud-pane</div>,
}));

vi.mock("@/components/settings/LocalModelPane", () => ({
  LocalModelPane: () => <div>local-pane</div>,
}));

const setBg = vi.fn();

beforeEach(() => {
  setBg.mockReset();
  setBg.mockResolvedValue(undefined);
  useLlmStore.setState({
    loaded: true,
    provider: "ollama",
    activeId: "ollama:qwen3.5:2b",
    localModel: "qwen3.5:2b",
    backgroundLocalOnly: false,
    sidecarUp: true,
    installed: [],
    catalog: [],
    pulls: {},
    testingId: null,
    lastError: null,
    modelTab: "local",
    cloudEntries: [],
    setBackgroundLocalOnly: setBg,
  });
});

describe("ModelSection routing toggle", () => {
  it("展示后台钉本地开关且默认关", () => {
    render(<ModelSection />);
    const row = screen.getByTestId("routing-toggle");
    expect(row).toBeTruthy();
    const toggle = screen.getByRole("checkbox");
    expect((toggle as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText(/夜间记忆整理、总结与周报/)).toBeTruthy();
  });

  it("拨动开关调用 setBackgroundLocalOnly", async () => {
    render(<ModelSection />);
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() => {
      expect(setBg).toHaveBeenCalledWith(true);
    });
  });
});
