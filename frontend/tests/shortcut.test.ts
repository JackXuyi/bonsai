import { describe, expect, it } from "vitest";
import { ROUTES, routeForKeys, titleForPath } from "@/lib/shortcut";

describe("shortcut", () => {
  it("⌘1–⌘5 → 五条路由映射（1=对话 固定最前）", () => {
    expect(routeForKeys(true, "1")).toBe("/");
    expect(routeForKeys(true, "2")).toBe("/face");
    expect(routeForKeys(true, "3")).toBe("/tools");
    expect(routeForKeys(true, "4")).toBe("/memory");
    expect(routeForKeys(true, "5")).toBe("/settings");
    expect(ROUTES).toHaveLength(5);
    expect(ROUTES[0]).toEqual({ path: "/", label: "对话" });
  });

  it("无修饰键 / 非数字键不触发；标题映射回落", () => {
    expect(routeForKeys(false, "1")).toBeNull();
    expect(routeForKeys(true, "a")).toBeNull();
    expect(routeForKeys(true, "6")).toBeNull();
    expect(routeForKeys(true, "F1")).toBeNull();

    expect(titleForPath("/")).toBe("对话");
    expect(titleForPath("/memory")).toBe("记忆");
    expect(titleForPath("/unknown")).toBe("阿盆");
  });
});
