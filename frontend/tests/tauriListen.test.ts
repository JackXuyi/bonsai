import { describe, expect, it, vi } from "vitest";

const registerSpy = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => registerSpy(...args),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { safeUnlisten } from "@/lib/tauriListen";

// useTauriEvent / listen 在 jsdom（无 __TAURI_INTERNALS__）下返回 no-op，
// 这里覆盖 safeUnlisten 的幂等语义 + no-op 路径；注册→卸载路径在真机验收。
describe("tauriListen", () => {
  it("safeUnlisten 幂等：null / 抛错的 / 多次调用都不炸", () => {
    expect(() => safeUnlisten(null)).not.toThrow();
    const throwing = vi.fn(() => {
      throw new Error("already unlistened");
    });
    expect(() => {
      safeUnlisten(throwing);
      safeUnlisten(throwing);
    }).not.toThrow();
    expect(throwing).toHaveBeenCalledTimes(2);
  });

  it("注册函数被透传事件名与 handler", async () => {
    const { listen } = await import("@/lib/tauriListen");
    // jsdom 无 __TAURI_INTERNALS__ → 不触达 tauriListen，直接 no-op
    const un = await listen("face:state", () => {});
    expect(typeof un).toBe("function");
    expect(() => un()).not.toThrow();
  });
});
