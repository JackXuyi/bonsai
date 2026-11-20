import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CloudSection } from "@/components/settings/CloudSection";
import { useLlmStore } from "@/lib/stores/llmStore";
import { useUiStore } from "@/lib/stores/uiStore";
import type { CloudEntryView } from "@/lib/types";

const entry: CloudEntryView = {
  id: "c1",
  name: "DeepSeek",
  kind: "custom",
  baseUrl: "https://api.deepseek.com/v1",
  model: "deepseek-chat",
  maskedKey: "••••9f3a",
};

let activateCloud: ReturnType<typeof vi.fn>;

beforeEach(() => {
  activateCloud = vi.fn().mockResolvedValue(true);
  useLlmStore.setState({
    activeId: "ollama:qwen3.5:2b",
    testingId: null,
    lastError: null,
    cloudEntries: [entry],
    activateCloud,
  } as never);
  useUiStore.setState({ cloudOptinAcked: false });
});

function clickCard() {
  fireEvent.click(screen.getByText("DeepSeek"));
}

describe("CloudSection opt-in", () => {
  it("首次选中弹警示，且不立刻切换", () => {
    render(<CloudSection />);
    clickCard();
    expect(screen.getByRole("dialog", { name: "切换到云端模型" })).toBeTruthy();
    expect(activateCloud).not.toHaveBeenCalled();
  });

  it("本会话已确认过则直接切换，不再弹窗", async () => {
    useUiStore.setState({ cloudOptinAcked: true });
    render(<CloudSection />);
    clickCard();
    expect(screen.queryByRole("dialog", { name: "切换到云端模型" })).toBeNull();
    await waitFor(() => expect(activateCloud).toHaveBeenCalledWith("c1"));
  });

  it("点「还是本地」关窗且不切换；确认后才切", async () => {
    render(<CloudSection />);
    clickCard();
    fireEvent.click(screen.getByRole("button", { name: "还是本地" }));
    expect(screen.queryByRole("dialog", { name: "切换到云端模型" })).toBeNull();
    expect(activateCloud).not.toHaveBeenCalled();
    expect(useUiStore.getState().cloudOptinAcked).toBe(false);

    clickCard();
    fireEvent.click(screen.getByRole("button", { name: "我知道了，切" }));
    await waitFor(() => expect(activateCloud).toHaveBeenCalledWith("c1"));
    expect(useUiStore.getState().cloudOptinAcked).toBe(true);
  });
});
