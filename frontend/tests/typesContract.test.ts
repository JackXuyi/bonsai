import { describe, expect, it } from "vitest";
import type {
  DialogueOutcome,
  MailStatus,
  McpServerRow,
  StoreLockStats,
} from "@/lib/types";

/**
 * 最小契约：前端类型字段名与 Rust serde 约定对齐（snake / camel 混用处钉死）。
 * 不做运行时 invoke；防止 barrel 拆分后漏导出或字段改名静默漂。
 */
describe("types contract (AGENTS §12 barrel)", () => {
  it("导出诊断与邮件关键 payload 键", () => {
    const mail: MailStatus = {
      enabled: false,
      supported: true,
      today_reads: 0,
      today_drafts: 0,
      today_marks: 0,
      note: "默认关闭",
    };
    expect(Object.keys(mail).sort()).toEqual(
      [
        "enabled",
        "note",
        "supported",
        "today_drafts",
        "today_marks",
        "today_reads",
      ].sort()
    );

    const locks: StoreLockStats = {
      write_acquires: 0,
      write_wait_ms_total: 0,
      write_wait_slow: 0,
      write_hold_ms_total: 0,
      write_hold_samples: 0,
      write_wait_ms_avg: 0,
      write_hold_ms_avg: 0,
    };
    expect(locks.write_wait_slow).toBe(0);
  });

  it("对话结果与 MCP 行保持既有命名", () => {
    const out: DialogueOutcome = {
      reply: "hi",
      filter: { hedged_disagreement: false },
      model_used: false,
      crisis: false,
    };
    expect("model_used" in out).toBe(true);

    const row: McpServerRow = {
      id: "1",
      name: "x",
      transport: "http",
      args: [],
      headers: [],
      env: [],
      enabled: false,
      toolCount: 0,
    };
    expect("toolCount" in row).toBe(true);
  });
});
