import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { PersonaCard, MODE_LABEL } from "@/components/tools/PersonaCard";
import type { PersonaSnapshot } from "@/lib/types";

const snapMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return { ...orig, personaSnapshot: () => snapMock() };
});

// jsdom 没有 Tauri 运行时，事件监听 mock 成 no-op
vi.mock("@/lib/tauriListen", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/tauriListen")>();
  return { ...orig, useTauriEvent: () => undefined };
});

function fixture(mode: PersonaSnapshot["relationshipMode"]): PersonaSnapshot {
  return {
    card: {
      name: "大肥鱼",
      tone: "回复偏慢偏短，先想后说",
      constitutionItems: ["宁可说不知道，也不编", "不评价用户的外貌"],
      bannedForms: ["虽然很有道理，但"],
    },
    relationshipMode: mode,
    relationshipStage: 0,
    faceState: "calm",
    growthStage: 0,
    dormant: false,
  };
}

beforeEach(() => {
  snapMock.mockReset();
  snapMock.mockResolvedValue(fixture("rusty"));
});

describe("PersonaCard", () => {
  it("模式中文映射：rusty → 生疏；reserved → 拘谨", async () => {
    render(<PersonaCard />);
    expect(await screen.findByText("生疏")).toBeTruthy();
    expect(screen.getByTestId("persona-card").textContent).toContain("换模型只换表达，不换本体");

    snapMock.mockResolvedValue(fixture("reserved"));
    render(<PersonaCard />);
    expect(await screen.findByText("拘谨")).toBeTruthy();
  });

  it("MODE_LABEL 全档位映射齐", () => {
    expect(MODE_LABEL.reserved).toBe("拘谨");
    expect(MODE_LABEL.familiar).toBe("熟络");
    expect(MODE_LABEL.direct).toBe("会顶嘴");
    expect(MODE_LABEL.rusty).toBe("生疏");
  });
});
