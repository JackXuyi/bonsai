import { describe, expect, it } from "vitest";
import { parseToolArgLines, toolArgsPreview } from "@/lib/formatToolArgs";

describe("formatToolArgs", () => {
  it("拆分后端键值行", () => {
    const lines = parseToolArgLines("路径：/tmp/a.md\n内容预览：hi");
    expect(lines).toEqual([
      { key: "路径", value: "/tmp/a.md" },
      { key: "内容预览", value: "hi" },
    ]);
  });

  it("JSON 对象按键排序展开", () => {
    const lines = parseToolArgLines('{"city":"杭州","auth":"x"}');
    expect(lines.map((l) => l.key)).toEqual(["auth", "city"]);
  });

  it("预览压成单行并截断", () => {
    expect(toolArgsPreview("a\n  b", 10)).toBe("a b");
    expect(toolArgsPreview("x".repeat(50), 10).endsWith("…")).toBe(true);
  });
});
