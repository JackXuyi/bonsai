import { beforeEach, describe, expect, it } from "vitest";
import {
  normalizeStepStatus,
  progressFromRow,
  usePlanStore,
} from "@/lib/stores/planStore";
import type { PlanTaskRow } from "@/lib/types";

function row(over: Partial<PlanTaskRow> = {}): PlanTaskRow {
  return {
    id: "t1",
    createdAt: "2026-11-23T10:00:00Z",
    goal: "查日程再写摘要",
    status: "running",
    toolCalls: 0,
    finalReply: null,
    plan: {
      version: 1,
      goal: "查日程再写摘要",
      replans: 0,
      steps: [
        { seq: 0, intent: "查今天的日程", tool_hint: "calendar_list", status: "pending", attempts: 0, tool_call_id: null },
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
});

describe("normalizeStepStatus", () => {
  it("两套词都收敛到同一组，未知值按 pending", () => {
    expect(normalizeStepStatus("ok")).toBe("done");
    expect(normalizeStepStatus("denied")).toBe("skipped");
    expect(normalizeStepStatus("error")).toBe("failed");
    expect(normalizeStepStatus("done")).toBe("done");
    expect(normalizeStepStatus("running")).toBe("running");
    expect(normalizeStepStatus("")).toBe("pending");
    expect(normalizeStepStatus("weird")).toBe("pending");
  });
});

describe("progressFromRow", () => {
  it("plan_json 的 snake_case 映射到视图模型并按 seq 排序", () => {
    const p = progressFromRow(
      row({
        plan: {
          goal: "目标",
          steps: [
            { seq: 1, intent: "第二步", tool_hint: "fs_write", status: "done", attempts: 2, tool_call_id: 7 },
            { seq: 0, intent: "第一步", status: "ok" },
          ],
        },
      }),
    );
    expect(p.goal).toBe("目标");
    expect(p.steps.map((s) => s.seq)).toEqual([0, 1]);
    expect(p.steps[1].status).toBe("done");
    expect(p.steps[1].attempts).toBe(2);
    expect(p.steps[1].toolCallId).toBe(7);
  });
});

describe("applyTaskEvent", () => {
  it("事件乱序也按 seq 落位（seq 2 先到、seq 1 后到）", () => {
    const s = usePlanStore.getState();
    s.applyTaskEvent({ event: "task_start", id: "t1", goal: "查日程再写摘要", planned: true });
    s.applyTaskEvent({ event: "task_step", id: "t1", seq: 2, tool: "fs_write", status: "ok" });
    s.applyTaskEvent({ event: "task_step", id: "t1", seq: 1, tool: "fs_read", status: "ok" });
    const cur = usePlanStore.getState().current!;
    expect(cur.goal).toBe("查日程再写摘要");
    expect(cur.status).toBe("running");
    expect(cur.steps.map((x) => x.seq)).toEqual([1, 2]);
    expect(cur.steps.every((x) => x.status === "done")).toBe(true);
  });

  it("同一事件重复到达不去重成两行（幂等）", () => {
    const s = usePlanStore.getState();
    s.applyTaskEvent({ event: "task_start", id: "t1", goal: "g" });
    for (let i = 0; i < 3; i += 1) {
      s.applyTaskEvent({ event: "task_step", id: "t1", seq: 0, tool: "calendar_list", status: "ok" });
    }
    expect(usePlanStore.getState().current!.steps).toHaveLength(1);
    expect(usePlanStore.getState().history).toHaveLength(1);
  });

  it("迟到事件不降级终态（A14：实时候选不覆盖回放权威态）", () => {
    usePlanStore.getState().setHistory([
      row({
        status: "done",
        plan: {
          goal: "查日程再写摘要",
          steps: [
            { seq: 0, intent: "查今天的日程", status: "done", attempts: 1, tool_call_id: 11 },
            { seq: 1, intent: "读待办文件", status: "done", attempts: 1, tool_call_id: 12 },
            { seq: 2, intent: "写一段摘要", status: "done", attempts: 1, tool_call_id: 13 },
          ],
        },
      }),
    ]);
    usePlanStore.getState().applyTaskEvent({
      event: "task_step", id: "t1", seq: 0, tool: "calendar_list", status: "running",
    });
    const cur = usePlanStore.getState().current!;
    expect(cur.status).toBe("done");
    expect(cur.steps[0].status).toBe("done");
    expect(cur.steps[0].toolCallId).toBe(11);
  });

  it("取消：终止态 + 未跑步骤标 skipped + 留原因", () => {
    const s = usePlanStore.getState();
    s.applyTaskEvent({ event: "task_start", id: "t1", goal: "g" });
    s.applyTaskEvent({ event: "task_step", id: "t1", seq: 0, tool: "fs_read", status: "ok" });
    s.applyTaskEvent({ event: "task_canceled", reason: "user" });
    const cur = usePlanStore.getState().current!;
    expect(cur.status).toBe("canceled");
    expect(cur.cancelReason).toBe("user");
    expect(cur.steps[0].status).toBe("done");
  });

  it("脏事件/未知事件一律忽略（面板不猜）", () => {
    const s = usePlanStore.getState();
    s.applyTaskEvent(null);
    s.applyTaskEvent({});
    s.applyTaskEvent({ id: "t1" });
    s.applyTaskEvent({ event: "task_step", id: "t1", seq: -1 });
    s.applyTaskEvent({ event: "mystery", id: "t1" });
    expect(usePlanStore.getState().current).toBeNull();
    expect(usePlanStore.getState().history).toEqual([]);
  });
});

describe("setHistory", () => {
  it("回放态补语义、实时态保住进度（intent 来自库，status 只升阶）", () => {
    const s = usePlanStore.getState();
    s.applyTaskEvent({ event: "task_start", id: "t1", goal: "查日程再写摘要" });
    s.applyTaskEvent({ event: "task_step", id: "t1", seq: 1, tool: "fs_read", status: "ok" });
    s.setHistory([row()]);
    const cur = usePlanStore.getState().current!;
    expect(cur.steps).toHaveLength(3);
    expect(cur.steps[1].intent).toBe("读待办文件");
    expect(cur.steps[1].status).toBe("done");
    expect(cur.steps[0].status).toBe("pending");
  });

  it("执行中的计划优先当主角，否则取最近一条", () => {
    usePlanStore.getState().setHistory([
      row({ id: "new", status: "done", plan: { goal: "刚说完的", steps: [{ seq: 0, intent: "写", status: "done" }] } }),
      row({ id: "old", status: "running", plan: { goal: "还在跑", steps: [{ seq: 0, intent: "查", status: "done" }] } }),
    ]);
    const st = usePlanStore.getState();
    expect(st.current?.id).toBe("old");
    expect(st.history.map((p) => p.id)).toEqual(["new", "old"]);
  });

  it("单步任务（plan_mode=none 且无 plan）不进面板", () => {
    usePlanStore.getState().setHistory([
      row({ id: "single", status: "done", plan: null, planMode: "none" }),
    ]);
    const st = usePlanStore.getState();
    expect(st.current).toBeNull();
    expect(st.history).toEqual([]);
  });
});
