import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { classifyLine } from "@/lib/logLines";

describe("classifyLine", () => {
  it("分级：error / warn / info 命中大写级别词", () => {
    expect(classifyLine("21:05:51 [ERROR bonsai] sidecar spawn failed")).toBe("error");
    expect(classifyLine("21:05:51 [WARN llm::serve] 11435 busy, reuse")).toBe("warn");
    expect(classifyLine("21:05:51 [INFO bonsai] consolidate: 3 events consumed")).toBe("info");
  });

  it("其余行都是 plain（ollama 自身输出、空行）", () => {
    expect(classifyLine("time=2026-11-19T21:05:51.000+08:00 status=ok")).toBe("plain");
    expect(classifyLine("")).toBe("plain");
    expect(classifyLine("[warn] 小写不算")).toBe("plain");
  });
});
