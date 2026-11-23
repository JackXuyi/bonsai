import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

// inTauri 在模块加载时求值，必须先于导入定义 Tauri 运行时标记
vi.hoisted(() => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: () => Promise.resolve(null) },
    writable: true,
  });
});

import { PlanProgressCard } from "@/components/tools/PlanProgressCard";
import { usePlanStore } from "@/lib/stores/planStore";
import type { PlanTaskRow } from "@/lib/types";

const listMock = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return { ...orig, planList: (limit?: number) => listMock(limit) };
});

function planRow(over: Partial<PlanTaskRow> = {}): PlanTaskRow {
  return {
    id: "t1",
    createdAt: "2026-11-23T10:00:00Z",
    goal: "查日程再写摘要",
    status: "running",
    toolCalls: 1,
    finalReply: null,
    plan: {
      goal: "查日程再写摘要",
      steps: [
        { seq: 0, intent: "查今天的日程", tool_hint: "calendar_list", status: "done", attempts: 1, tool_call_id: 11 },
        { seq: 1, intent: "读待办文件", tool_hint: "fs_read", status: "pending", attempts: 0, tool_call_id: null },
        { seq: 2, intent: "写一段摘要", tool_hint: "fs_write", status: "pending", attempts: 0, tool_call_id: null },
      ],
    },
    planMode: "planned",
    cancelReason: null,
    confirmMode: "auto",
    ...over,
  };
}

beforeEach(() => {
  usePlanStore.setState({ current: null, history: [] });
  listMock.mockReset();
  listMock.mockResolvedValue([]);
});

describe("PlanProgressCard", () => {
  it("首屏回放：目标 + 每步 intent + 进度计数（A14 基线）", async () => {
    listMock.mockResolvedValue([planRow()]);
    render(<PlanProgressCard active />);
    expect(await screen.findByText("查日程再写摘要")).toBeTruthy();
    expect(screen.getByText("查今天的日程")).toBeTruthy();
    expect(screen.getByText("读待办文件")).toBeTruthy();
    expect(screen.getByText("写一段摘要")).toBeTruthy();
    expect(screen.getByText("1/3 步")).toBeTruthy();
    const bar = screen.getByRole("progressbar", { name: "计划进度" });
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("3");
  });

  it("实时推进：task_step 到达后步骤徽标与计数跟着走（A13）", async () => {
    listMock.mockResolvedValue([planRow()]);
    render(<PlanProgressCard active />);
    await screen.findByText("查日程再写摘要");

    act(() => {
      usePlanStore.getState().applyTaskEvent({
        event: "task_step", id: "t1", seq: 1, tool: "fs_read", status: "ok",
      });
    });
    await waitFor(() => expect(screen.getByText("2/3 步")).toBeTruthy());

    act(() => {
      usePlanStore.getState().applyTaskEvent({
        event: "task_step", id: "t1", seq: 2, tool: "fs_write", status: "error",
      });
    });
    expect(screen.getByText("2/3 步")).toBeTruthy();
    expect(screen.getByText("失败")).toBeTruthy();
  });

  it("只读：没有勾选框/输入框（过程不可在面板里改）", async () => {
    listMock.mockResolvedValue([planRow()]);
    render(<PlanProgressCard active />);
    await screen.findByText("查日程再写摘要");
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
  });

  it("历史计划折叠展示，点开后出现旧计划摘要", async () => {
    listMock.mockResolvedValue([
      planRow(),
      planRow({
        id: "t0",
        status: "canceled",
        plan: { goal: "昨天的两步活", steps: [{ seq: 0, intent: "查", status: "done" }] },
      }),
    ]);
    render(<PlanProgressCard active />);
    await screen.findByText("查日程再写摘要");
    fireEvent.click(screen.getByText("查看历史（1）"));
    expect(screen.getByText("昨天的两步活")).toBeTruthy();
    expect(screen.getByText("1/1")).toBeTruthy();
    expect(screen.getByText("已取消")).toBeTruthy();
  });

  it("空态给引导文案，不放假数据", async () => {
    render(<PlanProgressCard active />);
    expect(
      await screen.findByText("还没有计划——多步的事（先查再写、先看再改）它会先列个计划"),
    ).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });
});
