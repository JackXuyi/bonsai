import { describe, expect, it } from "vitest";
import {
  LEGACY_PATH_TO_SECTION,
  SETTINGS_SECTIONS,
  sectionById,
  settingsGroups,
  settingsHref,
} from "@/lib/settingsSections";

describe("settingsSections", () => {
  it("七分区齐全且分组顺序稳定", () => {
    expect(SETTINGS_SECTIONS.map((s) => s.id)).toEqual([
      "presence",
      "persona",
      "memory",
      "model",
      "agent",
      "access",
      "ops",
    ]);
    const groups = settingsGroups();
    expect(groups.map((g) => g.group)).toEqual(["我与它", "智能", "系统"]);
    expect(groups[0].items.map((i) => i.id)).toEqual([
      "presence",
      "persona",
      "memory",
    ]);
  });

  it("旧路由映射与 href / sectionById", () => {
    expect(LEGACY_PATH_TO_SECTION["/face"]).toBe("presence");
    expect(LEGACY_PATH_TO_SECTION["/tools"]).toBe("access");
    expect(LEGACY_PATH_TO_SECTION["/memory"]).toBe("memory");
    expect(settingsHref()).toBe("/settings");
    expect(settingsHref("model")).toBe("/settings?section=model");
    expect(sectionById("memory")?.label).toBe("记忆");
    expect(sectionById("nope")).toBeNull();
    expect(sectionById(null)).toBeNull();
  });
});
