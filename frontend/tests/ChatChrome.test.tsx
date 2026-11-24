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
  it("左会话、右设置再头像：设置链到 /settings，会话可翻抽屉", () => {
    const { container } = render(<ChatChrome />);

    expect(screen.getByLabelText("会话")).toBeTruthy();
    expect(screen.getByLabelText("设置").getAttribute("href")).toBe("/settings");
    expect(screen.getByText("阿盆")).toBeTruthy();

    const end = container.querySelector(".chat-chrome-end");
    expect(end).toBeTruthy();
    const endKids = Array.from(end!.children);
    expect(endKids[0].getAttribute("aria-label")).toBe("设置");
    expect(endKids[1].classList.contains("presence-badge")).toBe(true);
    // v3：顶栏用 xs 圆头像，不是 sm 立绘尺度
    expect(container.querySelector(".face-xs")).toBeTruthy();
    expect(container.querySelector(".face-sm")).toBeNull();

    fireEvent.click(screen.getByLabelText("会话"));
    expect(useUiStore.getState().sessionDrawerOpen).toBe(true);
  });
});
