import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { TraceView } from "@/components/trace/TraceView";
import { PromptBlock, parsePromptMessages } from "@/components/trace/TraceTimeline";
import type { TurnTrace } from "@/lib/types";

const getMock = vi.fn();
const findMock = vi.fn();
vi.mock("@/lib/commands", () => ({
  turnTraceGet: (...args: unknown[]) => getMock(...args),
  turnTraceFindByEvent: (...args: unknown[]) => findMock(...args),
}));

function trace(partial?: Partial<TurnTrace>): TurnTrace {
  return {
    id: 412,
    sessionId: "default",
    userEventId: 900,
    assistantEventId: 901,
    kind: "tool",
    modelUsed: true,
    toolLabel: "builtin:agenda_tomorrow",
    filterReport: '{"explicit":false,"hedged":false,"unconditional_affirmation":false}',
    durationMs: 6800,
    userText: "帮我看看明天上午有什么安排",
    replyText: "明天上午有两条。",
    ts: "2026-11-20T14:32:00+08:00",
    steps: [
      { seq: 1, stepKind: "crisis_check", title: "危机拦截检查", outcome: "miss", ts: "2026-11-20T14:32:00+08:00" },
      {
        seq: 2,
        stepKind: "llm_round",
        title: "第 1 轮 · 能力候选",
        prompt: "SYS PROMPT 全文",
        output: '{"candidates":["agenda_tomorrow"]}',
        outcome: "answered",
        durationMs: 1900,
        userText: "帮我看看明天上午有什么安排",
        ts: "2026-11-20T14:32:01+08:00",
      },
      {
        seq: 3,
        stepKind: "tool_call",
        title: "工具调用 · builtin:agenda_tomorrow",
        argsJson: '{"date_offset":1}',
        summary: "明天上午 2 条日程",
        outcome: "ok",
        durationMs: 41,
        ts: "2026-11-20T14:32:03+08:00",
      },
    ],
    ...partial,
  };
}

describe("TraceView（spec-s8 追踪页）", () => {
  beforeEach(() => {
    getMock.mockReset().mockResolvedValue(null);
    findMock.mockReset().mockResolvedValue(null);
  });

  it("渲染摘要卡（kind 徽标 / 用户输入 / 回复）与步骤数", async () => {
    getMock.mockResolvedValue(trace());
    render(<TraceView turnId={412} />);
    await screen.findByText("工具回合");
    expect(screen.getByText(/帮我看看明天上午有什么安排/)).toBeTruthy();
    expect(screen.getByText(/明天上午有两条。/)).toBeTruthy();
    expect(screen.getByText(/3 步/)).toBeTruthy();
    expect(screen.getByText(/总耗时/)).toBeTruthy();
    expect(screen.getByText(/过滤通过/)).toBeTruthy();
    // getMock 直接用 turnId，不需要反查
    expect(getMock).toHaveBeenCalledWith(412);
    expect(findMock).not.toHaveBeenCalled();
  });

  it("查不到 trace → 空态文案", async () => {
    getMock.mockResolvedValue(null);
    render(<TraceView turnId={1} />);
    await screen.findByText("没有留下记录");
  });

  it("只有 eventId 时先反查再取 trace", async () => {
    findMock.mockResolvedValue(77);
    getMock.mockResolvedValue(trace({ id: 77 }));
    render(<TraceView eventId={9001} />);
    await screen.findByText("工具回合");
    expect(findMock).toHaveBeenCalledWith(9001);
    expect(getMock).toHaveBeenCalledWith(77);
  });

  it("展开 llm_round 看 prompt 全文；tool_call 看参数与返回摘要", async () => {
    getMock.mockResolvedValue(trace());
    render(<TraceView turnId={412} />);
    // 默认折叠：prompt 全文不可见，首行摘要在
    await screen.findByText(/输出：\{"candidates"/);
    expect(screen.queryByText("SYS PROMPT 全文")).toBeNull();

    // 展开 LLM 轮
    fireEvent.click(screen.getByText(/第 1 轮 · 能力候选/));
    expect(screen.getByText("SYS PROMPT 全文")).toBeTruthy();
    // 当前输入（user message 不在 system prompt 里，v11 起每步自带）
    expect(screen.getByText(/当前输入（user message/)).toBeTruthy();
    expect(screen.getAllByText("帮我看看明天上午有什么安排").length).toBeGreaterThan(0);

    // 展开工具步
    fireEvent.click(screen.getByText(/工具调用 · builtin:agenda_tomorrow/));
    expect(screen.getByText('{"date_offset":1}')).toBeTruthy();
    expect(screen.getByText("明天上午 2 条日程")).toBeTruthy();
  });

  it("工具返回是 memory_search JSON → 结构化卡片（窗口/行/记忆）", async () => {
    const memorySummary = JSON.stringify({
      window: "11月20日（周五）",
      dialogue_count: 2,
      fact_count: 1,
      lines: [
        { ts: "11-20 09:00", who: "用户", text: "水培该换水了吗" },
        { ts: "11-20 09:01", who: "盆栽", text: "一周一次就够。" },
      ],
      facts: ["用户在养水培植物"],
    });
    getMock.mockResolvedValue(
      trace({
        toolLabel: "builtin:memory_search",
        steps: [
          {
            seq: 1,
            stepKind: "tool_call",
            title: "工具调用 · memory_search",
            argsJson: '{"date_from":"2026-11-20","date_to":"2026-11-20"}',
            summary: memorySummary,
            outcome: "ok",
            durationMs: 5,
            ts: "2026-11-20T14:32:03+08:00",
          },
        ],
      })
    );
    render(<TraceView turnId={412} />);
    fireEvent.click(await screen.findByText(/工具调用 · memory_search/));
    // 结构化卡片：窗口头 + 行 + 记忆，而不是裸 JSON
    expect(screen.getByText("11月20日（周五）")).toBeTruthy();
    expect(screen.getByText(/对话 2 条 · 记忆 1 条/)).toBeTruthy();
    expect(screen.getByText("水培该换水了吗")).toBeTruthy();
    expect(screen.getByText("一周一次就够。")).toBeTruthy();
    expect(screen.getByText("用户在养水培植物")).toBeTruthy();
    expect(screen.queryByText(/"dialogue_count"/)).toBeNull();
  });

  it("工具返回是未知 JSON → pretty 打印兜底；非 JSON → 原样", async () => {
    getMock.mockResolvedValue(
      trace({
        steps: [
          {
            seq: 1,
            stepKind: "tool_call",
            title: "工具调用 · prune",
            argsJson: "{}",
            summary: '{"cpuPercent": 12, "benign": 115}',
            outcome: "ok",
            durationMs: 9,
            ts: "2026-11-20T14:32:03+08:00",
          },
        ],
      })
    );
    render(<TraceView turnId={412} />);
    fireEvent.click(await screen.findByText(/工具调用 · prune/));
    expect(screen.getByText(/"cpuPercent": 12/)).toBeTruthy();

    // 非 JSON 原样（旧 trace 数据兼容）
    getMock.mockResolvedValue(
      trace({
        steps: [
          {
            seq: 1,
            stepKind: "tool_call",
            title: "工具调用 · legacy",
            argsJson: "{}",
            summary: "杭州今天多云 18 度",
            outcome: "ok",
            durationMs: 9,
            ts: "2026-11-20T14:32:03+08:00",
          },
        ],
      })
    );
    render(<TraceView turnId={413} />);
    fireEvent.click(await screen.findByText(/工具调用 · legacy/));
    expect(screen.getByText("杭州今天多云 18 度")).toBeTruthy();
  });
});

describe("PromptBlock（prompt 数组结构化渲染，2026-11-22）", () => {
  it("parsePromptMessages：合法 ToolMsg[] 才解析，其余 null", () => {
    const good = JSON.stringify([
      { role: "system", content: "# 人格宪法" },
      { role: "user", content: "周四聊天原文" },
    ]);
    expect(parsePromptMessages(good)).toHaveLength(2);
    expect(parsePromptMessages(good)![0].role).toBe("system");
    // 非 JSON / 非 数组 / 字段缺失 / 空数组 → null
    expect(parsePromptMessages("本地路径纯文本 prompt")).toBeNull();
    expect(parsePromptMessages('{"role":"user"}')).toBeNull();
    expect(parsePromptMessages('[{"role":123,"content":"x"}]')).toBeNull();
    expect(parsePromptMessages("[]")).toBeNull();
  });

  it("数组形态：按角色分段渲染 + markdown 正文 + 复制原始全文", () => {
    const prompt = JSON.stringify([
      { role: "system", content: "# 环境\n不可变段" },
      { role: "user", content: "看 **周四** 聊天原文" },
      { role: "assistant", content: "好的。" },
    ]);
    render(<PromptBlock label="本轮 prompt（全文）" text={prompt} />);
    expect(screen.getByText("系统")).toBeTruthy();
    expect(screen.getByText("用户")).toBeTruthy();
    expect(screen.getByText("助手")).toBeTruthy();
    // markdown 渲染：# → 标题、** → strong
    expect(document.querySelector(".trace-prompt-body h1")?.textContent).toBe("环境");
    expect(document.querySelector(".trace-prompt-body strong")?.textContent).toBe("周四");
    expect(screen.queryByText(/"role":"system"/)).toBeNull();
  });

  it("非 JSON prompt（本地路径）→ PreBlock 原样兜底", () => {
    render(<PromptBlock label="本轮 prompt（全文）" text="SYS PROMPT 全文" />);
    expect(screen.getByText("SYS PROMPT 全文")).toBeTruthy();
    expect(document.querySelector(".trace-prompt")).toBeNull();
  });
});
