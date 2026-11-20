import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { EventList } from "@/components/memory/EventList";
import type { MemoryEventRow } from "@/lib/types";

function row(over: Partial<MemoryEventRow> = {}): MemoryEventRow {
  return {
    id: 1,
    ts: new Date().toISOString(),
    kind: "dialogue",
    content: "帮我看看今天的会",
    role: "user",
    derived: false,
    ...over,
  };
}

describe("EventList", () => {
  it("派生事件带 derived 类名，普通事件不带", () => {
    const { container } = render(
      <EventList
        rows={[
          row({ id: 1, kind: "dialogue" }),
          row({ id: 2, kind: "face_state", content: "focused", derived: true }),
        ]}
        byKind={[]}
      />
    );
    const rows = container.querySelectorAll(".mem-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].className).not.toContain("derived");
    expect(rows[1].className).toContain("derived");
    expect(rows[1].getAttribute("data-derived")).toBe("1");
  });

  it("时间本地化成「今天 HH:MM」，坏值不给时间", () => {
    const now = new Date();
    const iso = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 14, 2).toISOString();
    const { container } = render(<EventList rows={[row({ ts: iso, content: "x" })]} byKind={[]} />);
    expect(container.textContent).toContain("今天 14:02");

    const { container: broken } = render(
      <EventList rows={[row({ ts: "not-a-time", content: "y" })]} byKind={[]} />
    );
    expect(broken.querySelector(".mem-ts")?.textContent).toBe("");
  });

  it("空列表给一句人话而不是空白卡片", () => {
    const { container } = render(<EventList rows={[]} byKind={[]} />);
    expect(container.textContent).toContain("还没有事件");
  });

  it("kind 分组 chip 渲染计数", () => {
    const { container } = render(
      <EventList rows={[]} byKind={[{ kind: "dialogue", count: 2 }]} />
    );
    expect(container.textContent).toContain("dialogue 2");
  });
});
