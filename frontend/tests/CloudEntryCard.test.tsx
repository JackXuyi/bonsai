import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { CloudEntryCard } from "@/components/settings/CloudEntryCard";
import type { CloudEntryView } from "@/lib/types";

const entry: CloudEntryView = {
  id: "c1",
  name: "DeepSeek",
  kind: "custom",
  baseUrl: "https://api.deepseek.com/v1",
  model: "deepseek-chat",
  maskedKey: "••••9f3a",
};

describe("CloudEntryCard", () => {
  it("只显示脱敏 key 与端点，使用中打标签", () => {
    const { container } = render(
      <CloudEntryCard
        entry={entry}
        active
        testing={false}
        onPick={vi.fn()}
        onEdit={vi.fn()}
        onRemove={vi.fn()}
      />
    );
    const card = container.querySelector(".ccard") as HTMLElement;
    expect(card.className).toContain("on");
    expect(card.getAttribute("aria-pressed")).toBe("true");
    expect(card.textContent).toContain("••••9f3a");
    expect(card.textContent).not.toContain("sk-");
    expect(card.textContent).toContain("api.deepseek.com/v1");
    expect(card.textContent).toContain("使用中");
    expect(card.textContent).toContain("自定义兼容");
  });

  it("Claude 条目有独立标签（spec-m2.1）", () => {
    const { container } = render(
      <CloudEntryCard
        entry={{ ...entry, kind: "claude", baseUrl: "https://api.anthropic.com" }}
        active={false}
        testing={false}
        onPick={vi.fn()}
        onEdit={vi.fn()}
        onRemove={vi.fn()}
      />
    );
    const card = container.querySelector(".ccard") as HTMLElement;
    expect(card.textContent).toContain("Claude 官方");
    expect(card.textContent).not.toContain("自定义兼容");
  });

  it("点删除只删不选中（stopPropagation）", () => {
    const onPick = vi.fn();
    const onRemove = vi.fn();
    render(
      <CloudEntryCard
        entry={entry}
        active={false}
        testing={false}
        onPick={onPick}
        onEdit={vi.fn()}
        onRemove={onRemove}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "删除 DeepSeek" }));
    expect(onRemove).toHaveBeenCalledWith("c1");
    expect(onPick).not.toHaveBeenCalled();
  });
});
