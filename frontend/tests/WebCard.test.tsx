import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { WebCard } from "@/components/tools/WebCard";
import type { WebStatus } from "@/lib/types";

const statusMock = vi.fn();
const setConfigMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    webStatus: () => statusMock(),
    webSetConfig: (opts: { enabled?: boolean }) => setConfigMock(opts),
  };
});

// WebCard 里 toast 直接调 uiStore 的 action，jsdom 没有 ToastHost——
// mock 掉拿调用参数断言（FileCard 同款）。
vi.mock("@/lib/stores/uiStore", () => ({
  toast: (...args: unknown[]) => toastMock(...args),
  useUiStore: (sel: (s: { windowShownAt: number }) => unknown) =>
    sel({ windowShownAt: 0 }),
}));

function fixture(over: Partial<WebStatus> = {}): WebStatus {
  return { enabled: false, ...over };
}

beforeEach(() => {
  for (const m of [statusMock, setConfigMock, toastMock]) {
    m.mockReset();
  }
  statusMock.mockResolvedValue(fixture());
  setConfigMock.mockImplementation(async (opts: Partial<WebStatus>) =>
    fixture({ ...opts })
  );
});

describe("WebCard", () => {
  it("默认关：未开启提示 + 知情外发文案可见", async () => {
    render(<WebCard active />);
    await waitFor(() => expect(statusMock).toHaveBeenCalled());
    const card = screen.getByTestId("web-card");
    await waitFor(() =>
      expect(card.textContent).toContain("Web 检索未开启（默认关闭）")
    );
    // 知情同意的落点：外发说明在关闭态也可见（决定开不开之前就该知道）
    expect(card.textContent).toContain("发送给已配置的云端模型");
    expect(card.textContent).toContain("不会编造搜索结果");
  });

  it("开启后状态行切换；开关透传 setConfig", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    render(<WebCard active />);
    await waitFor(() =>
      expect(screen.getByTestId("web-card").textContent).toContain(
        "已开启 · 阿盆可以搜索网页、读取网页正文并总结"
      )
    );
    const toggle = screen
      .getByTestId("web-card")
      .querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    fireEvent.click(toggle);
    await waitFor(() => expect(setConfigMock).toHaveBeenCalledWith({ enabled: false }));
    // 关闭态回显
    await waitFor(() =>
      expect(screen.getByTestId("web-card").textContent).toContain("默认关闭")
    );
  });

  it("状态读取失败显式降级，不假装正常", async () => {
    statusMock.mockRejectedValue("boom");
    render(<WebCard active />);
    await waitFor(() =>
      expect(screen.getByTestId("web-card").textContent).toContain("状态读取失败")
    );
  });

  it("设置失败走 toast 提示", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    render(<WebCard active />);
    const toggle = await screen
      .findByTestId("web-card")
      .then((c) => c.querySelector('input[type="checkbox"]') as HTMLInputElement);
    setConfigMock.mockRejectedValue("boom");
    fireEvent.click(toggle);
    await waitFor(() => expect(toastMock).toHaveBeenCalledWith("Web 检索设置失败"));
  });
});
