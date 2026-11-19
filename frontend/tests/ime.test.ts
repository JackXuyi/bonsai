import { describe, expect, it } from "vitest";
import { shouldBlockSubmit } from "@/lib/ime";

describe("shouldBlockSubmit", () => {
  it("中文 IME 组合中的 Enter 不发送", () => {
    expect(shouldBlockSubmit(true, "Enter", false)).toBe(true);
  });

  it("普通 Enter 发送", () => {
    expect(shouldBlockSubmit(false, "Enter", false)).toBe(false);
  });

  it("Shift+Enter 换行", () => {
    expect(shouldBlockSubmit(false, "Enter", true)).toBe(true);
  });
});
