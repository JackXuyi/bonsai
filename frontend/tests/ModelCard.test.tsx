import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ModelCard } from "@/components/settings/ModelCard";

const row = { name: "qwen3.5:2b", sizeBytes: 800_000_000, modified: "2026-11-16T10:00:00Z" };

describe("ModelCard", () => {
  it("使用中：打 ✓ + on 类 + aria-pressed", () => {
    render(
      <ModelCard row={row} active testing={false} onPick={vi.fn()} onRemove={vi.fn()} />
    );
    const btn = screen.getByRole("button");
    expect(btn.className).toContain("on");
    expect(btn.getAttribute("aria-pressed")).toBe("true");
    expect(btn.textContent).toContain("使用中");
    expect(btn.textContent).toContain("0.8GB");
  });

  it("testing：spinner + disabled；think_ok=false 打 ⚠ 标签", () => {
    const { container } = render(
      <ModelCard
        row={row}
        meta={{
          tag: "qwen3.5:2b",
          sizeLabel: "0.8GB",
          sizeBytes: 800_000_000,
          note: "阿盆默认",
          thinkOk: false,
          recommended: false,
        }}
        active={false}
        testing
        onPick={vi.fn()}
        onRemove={vi.fn()}
      />
    );
    const btn = screen.getByRole("button");
    expect(btn.className).toContain("testing");
    expect((btn as HTMLButtonElement).disabled).toBe(true);
    expect(container.querySelector(".m-spin")).not.toBeNull();
    expect(btn.textContent).toContain("思维链不可关");
    expect(btn.className).not.toContain("on");
  });
});
