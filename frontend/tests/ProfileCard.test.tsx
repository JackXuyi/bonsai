import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { ProfileCard } from "@/components/tools/ProfileCard";
import type { ProfileEntry } from "@/lib/types";

const listMock = vi.fn();
const setMock = vi.fn();
const deleteMock = vi.fn();
const toastMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    profileList: () => listMock(),
    profileSet: (key: string, value: string) => setMock(key, value),
    profileDelete: (key: string) => deleteMock(key),
  };
});

vi.mock("@/lib/stores/uiStore", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/stores/uiStore")>();
  return { ...orig, toast: (msg: string) => toastMock(msg) };
});

function entry(key: string, value: string, source: ProfileEntry["source"]): ProfileEntry {
  return { key, value, source, confidence: 0.7, updatedAt: "2026-11-23T00:00:00Z" };
}

beforeEach(() => {
  listMock.mockReset();
  setMock.mockReset();
  deleteMock.mockReset();
  toastMock.mockReset();
  listMock.mockResolvedValue([]);
  deleteMock.mockResolvedValue(1);
  setMock.mockResolvedValue(true);
});

describe("ProfileCard", () => {
  it("渲染中文名 + 值 + 来源徽标", async () => {
    listMock.mockResolvedValue([
      entry("name", "许一", "manual"),
      entry("occupation", "QA / 质量管理", "rule"),
    ]);
    render(<ProfileCard active />);
    expect(await screen.findByText("许一")).toBeTruthy();
    expect(screen.getByText("QA / 质量管理")).toBeTruthy();
    expect(screen.getByText("你写的")).toBeTruthy();
    expect(screen.getByText("聊天里记的")).toBeTruthy();
    expect(screen.getByText("称呼")).toBeTruthy();
    expect(screen.getByText("职业")).toBeTruthy();
  });

  it("未知 key 丢弃（白名单前端兜底）", async () => {
    listMock.mockResolvedValue([
      entry("income", "50w", "model"),
      entry("name", "许一", "rule"),
    ]);
    render(<ProfileCard active />);
    await waitFor(() => expect(screen.getByText("许一")).toBeTruthy());
    expect(screen.queryByText("50w")).toBeNull();
    expect(screen.queryByText("收入")).toBeNull();
  });

  it("行内编辑：点改 → 输入 → 保存 → 调 profileSet + toast + 重拉", async () => {
    listMock.mockResolvedValue([entry("name", "许一", "rule")]);
    render(<ProfileCard active />);
    fireEvent.click(await screen.findByLabelText("修改称呼"));
    const input = screen.getByLabelText("修改称呼") as HTMLInputElement;
    expect(input.value).toBe("许一");
    fireEvent.change(input, { target: { value: "Jack" } });
    fireEvent.click(screen.getByText("保存"));
    await waitFor(() => expect(setMock).toHaveBeenCalledWith("name", "Jack"));
    expect(toastMock).toHaveBeenCalledWith("画像已更新");
    expect(listMock).toHaveBeenCalledTimes(2);
  });

  it("清除：点清 → 调 profileDelete", async () => {
    listMock.mockResolvedValue([entry("city", "杭州", "rule")]);
    render(<ProfileCard active />);
    fireEvent.click(await screen.findByLabelText("清除城市"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith("city"));
    expect(toastMock).toHaveBeenCalledWith("已清除这条画像");
  });

  it("读取失败显示 status-line；空态显示引导文案而非 DEMO", async () => {
    const { unmount } = render(<ProfileCard active />);
    await waitFor(() =>
      expect(screen.getByText("还没有任何画像——聊聊天它就慢慢认识你了")).toBeTruthy(),
    );
    unmount();

    listMock.mockRejectedValue(new Error("down"));
    render(<ProfileCard active />);
    await waitFor(() => expect(screen.getByText("用户画像读取失败")).toBeTruthy());
    expect(screen.queryByText(/Cursor/)).toBeNull();
  });
});
