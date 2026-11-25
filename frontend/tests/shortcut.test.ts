import { describe, expect, it } from "vitest";
import {
  routeForKeys,
  shortcutAction,
  titleForPath,
  normalizePath,
} from "@/lib/shortcut";

describe("shortcut v2", () => {
  it("⌘, → 设置；⌘⇧O → 会话抽屉；其它不触发", () => {
    expect(shortcutAction(true, false, ",")).toEqual({
      kind: "route",
      path: "/settings",
    });
    expect(shortcutAction(true, true, "o")).toEqual({ kind: "toggleSessions" });
    expect(shortcutAction(true, true, "O")).toEqual({ kind: "toggleSessions" });
    expect(shortcutAction(false, false, ",")).toBeNull();
    expect(shortcutAction(true, false, "1")).toBeNull();
    expect(shortcutAction(true, false, "o")).toBeNull();
  });

  it("routeForKeys 仅映射逗号（兼容包装）", () => {
    expect(routeForKeys(true, ",")).toBe("/settings");
    expect(routeForKeys(true, "1")).toBeNull();
  });

  it("标题与路径归一化", () => {
    expect(titleForPath("/")).toBe("对话");
    expect(titleForPath("/settings")).toBe("设置");
    expect(titleForPath("/trace?turn=1")).toBe("生成链路");
    expect(titleForPath("/unknown")).toBe("大肥鱼");
    expect(normalizePath("/index.html")).toBe("/");
    expect(normalizePath("/settings.html")).toBe("/settings");
  });
});
