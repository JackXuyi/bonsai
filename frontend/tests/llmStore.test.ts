import { beforeEach, describe, expect, it, vi } from "vitest";

// inTauri 在模块加载时判定 —— 先伪装 Tauri 环境，再动态 import store，
// 让 activate 走真实命令路径（而不是浏览器预览的模拟分支）。
Object.defineProperty(window, "__TAURI_INTERNALS__", {
  value: {},
  configurable: true,
});

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const { useLlmStore } = await import("@/lib/stores/llmStore");

const VIEW = {
  provider: "ollama" as const,
  localModel: "qwen3.5:4b",
  activeId: "ollama:qwen3.5:4b",
  sidecarUp: true,
};

beforeEach(() => {
  invokeMock.mockReset();
  useLlmStore.setState({
    loaded: true,
    provider: "ollama",
    activeId: "ollama:qwen3.5:2b",
    localModel: "qwen3.5:2b",
    sidecarUp: true,
    installed: [],
    catalog: [],
    pulls: {},
    testingId: null,
    lastError: null,
    modelTab: "local",
    cloudEntries: [],
  });
});

describe("llmStore", () => {
  it("activate 成功：更新 activeId/localModel + 页头模型 + 清 testing", async () => {
    invokeMock.mockResolvedValue(VIEW);
    const ok = await useLlmStore.getState().activate("qwen3.5:4b");

    expect(ok).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("llm_config_set_active", {
      id: "ollama:qwen3.5:4b",
    });
    const st = useLlmStore.getState();
    expect(st.activeId).toBe("ollama:qwen3.5:4b");
    expect(st.localModel).toBe("qwen3.5:4b");
    expect(st.testingId).toBeNull();
  });

  it("activate 失败：testing 态还原、activeId 不变、错误人话化", async () => {
    invokeMock.mockRejectedValue("probe_failed: timeout");
    const ok = await useLlmStore.getState().activate("qwen3.5:4b");

    expect(ok).toBe(false);
    const st = useLlmStore.getState();
    expect(st.testingId).toBeNull();
    expect(st.activeId).toBe("ollama:qwen3.5:2b");
    expect(st.lastError).toContain("模型加载失败");
  });

  it("applyPull 覆盖同一 tag 的进度快照", () => {
    const { applyPull } = useLlmStore.getState();
    applyPull({ tag: "gemma3:4b", status: "pulling", percent: 12, completed: 1, total: 8 });
    applyPull({ tag: "gemma3:4b", status: "pulling", percent: 48, completed: 4, total: 8 });
    applyPull({ tag: "llama3.2:3b", status: "warn_disk", percent: 0, completed: 0, total: 0 });

    const pulls = useLlmStore.getState().pulls;
    expect(pulls["gemma3:4b"].percent).toBe(48);
    expect(pulls["llama3.2:3b"].status).toBe("warn_disk");
  });

  it("modelTab 本地/云端来回切", () => {
    expect(useLlmStore.getState().modelTab).toBe("local");
    useLlmStore.getState().setModelTab("cloud");
    expect(useLlmStore.getState().modelTab).toBe("cloud");
    useLlmStore.getState().setModelTab("local");
    expect(useLlmStore.getState().modelTab).toBe("local");
  });

  it("activateCloud 走 cloud: 前缀 + 记录 provider", async () => {
    invokeMock.mockResolvedValue({
      ...VIEW,
      provider: "cloud",
      activeId: "cloud:c1",
      localModel: "qwen3.5:2b",
      maskedKey: "••••9f3a",
    });
    const ok = await useLlmStore.getState().activateCloud("c1");

    expect(ok).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("llm_config_set_active", { id: "cloud:c1" });
    const st = useLlmStore.getState();
    expect(st.provider).toBe("cloud");
    expect(st.activeId).toBe("cloud:c1");
  });

  it("clearPull 只清指定 tag", () => {
    const { applyPull, clearPull } = useLlmStore.getState();
    applyPull({ tag: "a", status: "done", percent: 100, completed: 1, total: 1 });
    applyPull({ tag: "b", status: "pulling", percent: 3, completed: 0, total: 1 });
    clearPull("a");

    const pulls = useLlmStore.getState().pulls;
    expect(pulls.a).toBeUndefined();
    expect(pulls.b.percent).toBe(3);
  });
});
