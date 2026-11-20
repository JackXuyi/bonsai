import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { LogsCard } from "@/components/settings/LogsCard";

const tailMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    debugLogTail: (...args: unknown[]) => tailMock(...args),
    debugOpenLogDir: vi.fn().mockResolvedValue(undefined),
  };
});

beforeEach(() => {
  tailMock.mockReset();
  tailMock.mockResolvedValue([]);
});

describe("LogsCard", () => {
  it("切 tab 立即拉对应来源", async () => {
    tailMock.mockImplementation((_file: string, _lines?: number) =>
      _file === "sidecar_log"
        ? Promise.resolve(["15:28:02 [INFO] llm sidecar ready on :11435"])
        : Promise.resolve([])
    );
    render(<LogsCard />);

    // 默认应用日志：暂无日志
    expect(await screen.findByText("暂无日志")).toBeTruthy();
    expect(tailMock).toHaveBeenCalledWith("app_log", 200);

    // 切到 sidecar：立即拉取，出日志行
    fireEvent.click(screen.getByRole("tab", { name: "sidecar 日志" }));
    expect(
      await screen.findByText("15:28:02 [INFO] llm sidecar ready on :11435")
    ).toBeTruthy();
    expect(tailMock).toHaveBeenLastCalledWith("sidecar_log", 200);
  });

  it("空文件显示「暂无日志」而非报错", async () => {
    render(<LogsCard />);
    expect(await screen.findByText("暂无日志")).toBeTruthy();
    // 失败也回落到空态，不弹红字
    tailMock.mockRejectedValue(new Error("boom"));
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    expect(await screen.findByText("暂无日志")).toBeTruthy();
  });
});
