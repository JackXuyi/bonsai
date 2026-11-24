import { describe, expect, it } from "vitest";
import {
  TOOL_ACCESS_LABEL,
  normalizeToolAccess,
} from "@/lib/toolAccess";

describe("normalizeToolAccess", () => {
  it("keeps known tiers", () => {
    expect(normalizeToolAccess("read")).toBe("read");
    expect(normalizeToolAccess("ask")).toBe("ask");
    expect(normalizeToolAccess("forbidden")).toBe("forbidden");
  });

  it("defaults unknown / missing to read", () => {
    expect(normalizeToolAccess(undefined)).toBe("read");
    expect(normalizeToolAccess(null)).toBe("read");
    expect(normalizeToolAccess("write")).toBe("read");
  });

  it("exposes Chinese labels for UI", () => {
    expect(TOOL_ACCESS_LABEL.read).toBe("只读");
    expect(TOOL_ACCESS_LABEL.ask).toBe("询问");
    expect(TOOL_ACCESS_LABEL.forbidden).toBe("禁止");
  });
});
