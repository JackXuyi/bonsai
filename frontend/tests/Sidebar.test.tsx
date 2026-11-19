import { beforeEach, describe, expect, it, vi } from "vitest";

// Sidebar 依赖：next/navigation 的 usePathname + PokeFace 链上的 tauri invoke
const pathnameMock = vi.fn();
vi.mock("next/navigation", () => ({
  usePathname: () => pathnameMock(),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(null),
}));

import { render, screen } from "@testing-library/react";
import { Sidebar } from "@/components/layout/Sidebar";

beforeEach(() => {
  pathnameMock.mockReset();
});

describe("Sidebar", () => {
  it("当前路由项高亮（aria-current），其余五项齐备", () => {
    pathnameMock.mockReturnValue("/memory");
    render(<Sidebar />);

    const items = screen.getAllByRole("link");
    expect(items).toHaveLength(5);
    expect(items.map((a) => a.textContent)).toEqual([
      "对话",
      "表情",
      "工具",
      "记忆",
      "设置",
    ]);

    const active = screen.getByText("记忆").closest("a")!;
    expect(active.getAttribute("aria-current")).toBe("page");
    expect(active.className).toContain("active");

    const chat = screen.getByText("对话").closest("a")!;
    expect(chat.getAttribute("aria-current")).toBeNull();
    expect(chat.className).not.toContain("active");
  });
});
