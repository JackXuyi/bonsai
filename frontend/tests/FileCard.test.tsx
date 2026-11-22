import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { FileCard } from "@/components/tools/FileCard";
import type { FsStatus } from "@/lib/types";

const statusMock = vi.fn();
const setConfigMock = vi.fn();
const rootAddMock = vi.fn();
const rootRemoveMock = vi.fn();
const pickMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    fsStatus: () => statusMock(),
    fsSetConfig: (opts: { enabled?: boolean; cloud_read?: boolean }) =>
      setConfigMock(opts),
    fsRootAdd: (path: string) => rootAddMock(path),
    fsRootRemove: (path: string) => rootRemoveMock(path),
    fsPickDirectory: () => pickMock(),
  };
});

// FileCard 里 toast 直接调 uiStore 的 action，jsdom 没有 ToastHost——
// mock 掉拿调用参数断言。
vi.mock("@/lib/stores/uiStore", () => ({
  toast: (...args: unknown[]) => toastMock(...args),
  useUiStore: (sel: (s: { windowShownAt: number }) => unknown) =>
    sel({ windowShownAt: 0 }),
}));

vi.mock("@/lib/tauriListen", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/tauriListen")>();
  return { ...orig, useTauriEvent: () => undefined };
});

function fixture(over: Partial<FsStatus> = {}): FsStatus {
  return { enabled: false, roots: [], cloud_read: true, ...over };
}

beforeEach(() => {
  for (const m of [statusMock, setConfigMock, rootAddMock, rootRemoveMock, pickMock, toastMock]) {
    m.mockReset();
  }
  statusMock.mockResolvedValue(fixture());
  setConfigMock.mockImplementation(async (opts: Partial<FsStatus>) =>
    fixture({ enabled: true, ...opts })
  );
});

describe("FileCard", () => {
  it("默认零授权：未开启提示 + 不渲染目录行", async () => {
    render(<FileCard active />);
    await waitFor(() => expect(statusMock).toHaveBeenCalled());
    const card = screen.getByTestId("file-card");
    expect(card.textContent).toContain("文件工具未开启（默认关闭）");
    expect(screen.queryByTestId("add-root")).toBeNull();
  });

  it("开启但零授权 → 引导文案；有授权目录时逐行列出并可移除", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    const { unmount } = render(<FileCard active />);
    await screen.findByTestId("add-root");
    expect(screen.getByTestId("file-card").textContent).toContain("还没授权任何目录");
    unmount();

    statusMock.mockResolvedValue(fixture({ enabled: true, roots: ["/Users/xuyi/Docs"] }));
    render(<FileCard active />);
    await screen.findByText("/Users/xuyi/Docs");
    const card = screen.getByTestId("file-card");
    expect(card.textContent).toContain("已授权 1 个目录");

    rootRemoveMock.mockResolvedValue(fixture({ enabled: true, roots: [] }));
    fireEvent.click(screen.getByTestId("remove-root-0"));
    await waitFor(() => expect(rootRemoveMock).toHaveBeenCalledWith("/Users/xuyi/Docs"));
  });

  it("添加流程：选择器 → rootAdd；选择器取消不触发 add", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    render(<FileCard active />);
    const add = await screen.findByTestId("add-root");

    pickMock.mockResolvedValue(null); // 用户取消
    fireEvent.click(add);
    await waitFor(() => expect(pickMock).toHaveBeenCalled());
    expect(rootAddMock).not.toHaveBeenCalled();

    pickMock.mockResolvedValue("/Users/xuyi/Exports");
    rootAddMock.mockResolvedValue(fixture({ enabled: true, roots: ["/Users/xuyi/Exports"] }));
    fireEvent.click(add);
    await waitFor(() => expect(rootAddMock).toHaveBeenCalledWith("/Users/xuyi/Exports"));
  });

  it("后端错误剥掉 code 前缀直接展示人话", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true }));
    render(<FileCard active />);
    const add = await screen.findByTestId("add-root");
    pickMock.mockResolvedValue("/Users/xuyi");
    rootAddMock.mockRejectedValue("fs_roots_max:最多 5 个目录");
    fireEvent.click(add);
    await waitFor(() => expect(rootAddMock).toHaveBeenCalled());
    await waitFor(() => expect(toastMock).toHaveBeenCalledWith("最多 5 个目录"));
  });

  it("cloud_read 开关透传 setConfig；关闭说明文案可见", async () => {
    statusMock.mockResolvedValue(fixture({ enabled: true, roots: ["/tmp/x"] }));
    render(<FileCard active />);
    await screen.findByTestId("add-root");
    expect(screen.getByTestId("file-card").textContent).toContain("不出这台电脑");
    setConfigMock.mockResolvedValue(fixture({ enabled: true, cloud_read: false }));
    const toggles = screen
      .getByTestId("file-card")
      .querySelectorAll('input[type="checkbox"]');
    // 第 0 个是总开关，第 1 个是 cloud_read
    fireEvent.click(toggles[1]);
    await waitFor(() => expect(setConfigMock).toHaveBeenCalledWith({ cloud_read: false }));
  });
});
