import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { invokeWithTimeout } from "@/lib/tauriInvoke";

beforeEach(() => {
  invokeMock.mockReset();
});

describe("invokeWithTimeout", () => {
  it("正常透传命令与参数", async () => {
    invokeMock.mockResolvedValue({ ok: 1 });
    const r = await invokeWithTimeout<{ ok: number }>("growth_status", { id: 7 });
    expect(r).toEqual({ ok: 1 });
    expect(invokeMock).toHaveBeenCalledWith("growth_status", { id: 7 });
  });

  it("超时拒绝（不挂死）", async () => {
    invokeMock.mockImplementation(() => new Promise(() => {}));
    await expect(
      invokeWithTimeout("slow_cmd", undefined, 30)
    ).rejects.toThrow(/\[IPC\] slow_cmd did not respond within 30ms/);
  });

  it("后端错误冒泡", async () => {
    invokeMock.mockRejectedValue(new Error("calendar_denied"));
    await expect(invokeWithTimeout("calendar_status")).rejects.toThrow(
      "calendar_denied"
    );
  });
});
