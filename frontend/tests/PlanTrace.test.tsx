import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import {
  PlanStateBlock,
  PlanSubmitBlock,
  parsePlanState,
  parseSubmittedPlan,
} from "@/components/trace/TraceTimeline";

describe("parseSubmittedPlan", () => {
  it("解析后端 PlanJson（snake_case），steps 保留审计 id", () => {
    const raw = JSON.stringify({
      goal: "查天气记笔记",
      plan_mode: "planned",
      steps: [
        { seq: 0, intent: "查天气", tool_hint: "weather", status: "done", attempts: 1, tool_call_id: 12 },
        { seq: 1, intent: "记笔记", tool_hint: "notes", status: "pending", attempts: 0, tool_call_id: null },
      ],
    });
    const plan = parseSubmittedPlan(raw);
    expect(plan?.goal).toBe("查天气记笔记");
    expect(plan?.steps).toHaveLength(2);
    expect(plan?.steps?.[0].tool_call_id).toBe(12);
    expect(plan?.steps?.[1].status).toBe("pending");
  });

  it("坏 JSON / 缺字段 / 非 object 一律回落 null（不炸）", () => {
    expect(parseSubmittedPlan("not json")).toBeNull();
    expect(parseSubmittedPlan("[]")).toBeNull();
    expect(parseSubmittedPlan("{}")).toBeNull();
    expect(parseSubmittedPlan(JSON.stringify({ goal: "x", steps: "no" }))).toBeNull();
  });
});

describe("parsePlanState", () => {
  it("解析步骤状态 JSON", () => {
    const st = parsePlanState(
      JSON.stringify({ seq: 1, status: "done", attempts: 2, tool_call_id: 34 })
    );
    expect(st?.seq).toBe(1);
    expect(st?.status).toBe("done");
    expect(st?.attempts).toBe(2);
    expect(st?.tool_call_id).toBe(34);
  });

  it("缺 status 回落 null", () => {
    expect(parsePlanState("garbage")).toBeNull();
    expect(parsePlanState(JSON.stringify({ seq: 0 }))).toBeNull();
  });
});

describe("计划卡渲染", () => {
  it("PlanSubmitBlock 显示目标与步骤状态", () => {
    const raw = JSON.stringify({
      goal: "整理下载目录",
      steps: [{ seq: 0, intent: "列文件", tool_hint: "fs_list", status: "done", tool_call_id: 5 }],
    });
    const { container } = render(<PlanSubmitBlock text={raw} />);
    const text = container.textContent ?? "";
    expect(text).toContain("整理下载目录");
    expect(text).toContain("列文件");
    expect(text).toContain("完成");
    expect(text).toContain("审计 #5");
  });

  it("PlanStateBlock 显示状态与关联审计 id", () => {
    const raw = JSON.stringify({ seq: 0, status: "failed", attempts: 2, tool_call_id: 9 });
    const { container } = render(<PlanStateBlock text={raw} />);
    const text = container.textContent ?? "";
    expect(text).toContain("失败");
    expect(text).toContain("尝试 2 次");
    expect(text).toContain("审计 #9");
  });

  it("坏 JSON 安全回落到原文块", () => {
    const { container } = render(<PlanSubmitBlock text="{broken" />);
    expect(container.textContent).toContain("{broken");
  });
});
