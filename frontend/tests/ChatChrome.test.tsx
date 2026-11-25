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

beforeEach(() => {
  useUiStore.setState({ sessionDrawerOpen: false, faceState: "calm" });
});

describe("ChatChrome", () => {
  it("左会话、右仅设置：设置链到 /settings，无 PresenceBadge", () => {
    const { container } = render(<ChatChrome />);

    expect(screen.getByLabelText("会话")).toBeTruthy();
    expect(screen.getByLabelText("设置").getAttribute("href")).toBe("/settings");
    expect(screen.queryByText("阿盆")).toBeNull();
    expect(container.querySelector(".presence-badge")).toBeNull();
    expect(container.querySelector(".face-xs")).toBeNull();

    const end = container.querySelector(".chat-chrome-end");
    expect(end).toBeTruthy();
    expect(end!.children).toHaveLength(1);
    expect(end!.children[0].getAttribute("aria-label")).toBe("设置");

    fireEvent.click(screen.getByLabelText("会话"));
    expect(useUiStore.getState().sessionDrawerOpen).toBe(true);
  });
});
