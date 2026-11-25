import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { applyTheme, readStoredThemeId, themeById, DEFAULT_THEME } from "@/lib/theme";

describe("theme", () => {
  beforeEach(() => {
    localStorage.removeItem("bonsai-theme");
    document.documentElement.removeAttribute("data-theme");
  });

  afterEach(() => {
    localStorage.removeItem("bonsai-theme");
  });

  it("默认 whale，applyTheme 写入 data-theme 与 CSS 变量", () => {
    expect(DEFAULT_THEME).toBe("whale");
    expect(themeById("nope").id).toBe("whale");
    applyTheme("moss");
    expect(document.documentElement.getAttribute("data-theme")).toBe("moss");
    expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#4c8c2b");
    expect(readStoredThemeId()).toBe("moss");
  });
});
