import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ChatChrome } from "@/components/layout/ChatChrome";
import { useUiStore } from "@/lib/stores/uiStore";

vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...rest
  }: {
    href: string;
    children: React.ReactNode;
    className?: string;
    "aria-label"?: string;
    title?: string;
  }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://localhost/${path}`,
  invoke: vi.fn().mockResolvedValue(null),
}));

beforeEach(() => {
  useUiStore.setState({ sessionDrawerOpen: false, faceState: "calm" });
});

describe("ChatChrome", () => {
  it("三槽：会话按钮翻抽屉、设置链到 /settings、存在感可见", () => {
    render(<ChatChrome />);

    expect(screen.getByLabelText("会话")).toBeTruthy();
    expect(screen.getByLabelText("设置").getAttribute("href")).toBe("/settings");
    expect(screen.getByText("阿盆")).toBeTruthy();

    fireEvent.click(screen.getByLabelText("会话"));
    expect(useUiStore.getState().sessionDrawerOpen).toBe(true);
  });
});
