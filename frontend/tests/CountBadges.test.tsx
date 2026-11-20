import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { CountBadges } from "@/components/memory/CountBadges";
import type { MemoryCounts } from "@/lib/types";

function counts(over: Partial<MemoryCounts> = {}): MemoryCounts {
  return {
    events: 0,
    eventsByKind: [],
    facts: 0,
    preferences: 0,
    boundaries: 0,
    predictionsTotal: 0,
    predictionsHit: 0,
    toolCalls: 0,
    calendarCache: 0,
    ...over,
  };
}

describe("CountBadges", () => {
  it("分母为 0 时显示「还没有预测」，不显示 NaN", () => {
    const { container } = render(<CountBadges counts={counts()} />);
    const text = container.textContent ?? "";
    expect(text).toContain("还没有预测");
    expect(text).not.toContain("NaN");
    expect(text).not.toContain("0/0");
  });

  it("有预测时给 命中/问过 的分数", () => {
    const { container } = render(
      <CountBadges counts={counts({ predictionsTotal: 12, predictionsHit: 7 })} />
    );
    expect(container.textContent).toContain("7/12");
  });

  it("七类计数都在场（事件/事实/偏好/边界/预测/工具/日历）", () => {
    const { container } = render(
      <CountBadges
        counts={counts({ events: 128, facts: 34, preferences: 12, boundaries: 5, toolCalls: 41, calendarCache: 9 })}
      />
    );
    const text = container.textContent ?? "";
    expect(text).toContain("128");
    expect(text).toContain("34");
    expect(text).toContain("边界 5");
    expect(text).toContain("41");
    expect(text).toContain("9");
  });
});
