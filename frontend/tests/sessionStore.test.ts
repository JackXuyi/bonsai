// 会话索引的纯函数（plan-context.md §8 列表行）：显示名兜底、相对日、分组。
import { describe, expect, it } from "vitest";
import {
  NEW_SESSION_TITLE,
  dayLabel,
  sessionTitle,
  splitSessions,
} from "@/lib/stores/sessionStore";
import type { SessionRow } from "@/lib/types";

const row = (id: string, archived = false): SessionRow => ({
  id,
  title: id,
  createdAt: "2026-11-01T10:00:00.000Z",
  updatedAt: "2026-11-01T10:00:00.000Z",
  archived,
  messageCount: 0,
  digest: null,
  digestUptoEventId: null,
});

describe("sessionTitle", () => {
  it("空标题 / 纯空白 / 行不存在都兜「新对话」", () => {
    expect(sessionTitle({ title: "" })).toBe(NEW_SESSION_TITLE);
    expect(sessionTitle({ title: "   \n " })).toBe(NEW_SESSION_TITLE);
    expect(sessionTitle(undefined)).toBe(NEW_SESSION_TITLE);
  });

  it("有标题就去空白原样用（后端已截断，不再二次加工）", () => {
    expect(sessionTitle({ title: "  封窗报价  " })).toBe("封窗报价");
  });
});

describe("dayLabel", () => {
  const now = new Date("2026-11-23T22:00:00");

  it("今天 / 昨天 / 更早（MM-DD）", () => {
    expect(dayLabel(new Date("2026-11-23T08:00:00").toISOString(), now)).toBe("今天");
    expect(dayLabel(new Date("2026-11-22T23:00:00").toISOString(), now)).toBe("昨天");
    expect(dayLabel(new Date("2026-11-05T12:00:00").toISOString(), now)).toBe("11-05");
  });

  it("跨月跨年按日历日算，不按 24 小时算", () => {
    const nye = new Date("2026-01-01T00:30:00");
    expect(dayLabel(new Date("2025-12-31T23:00:00").toISOString(), nye)).toBe("昨天");
  });

  it("坏时间戳返回空串（不渲染 NaN）", () => {
    expect(dayLabel("not-a-date", now)).toBe("");
  });
});

describe("splitSessions", () => {
  it("按 archived 分组，且保持后端给的顺序（已是最新在前）", () => {
    const list = [row("a"), row("b", true), row("c")];
    const { active, archived } = splitSessions(list);
    expect(active.map((r) => r.id)).toEqual(["a", "c"]);
    expect(archived.map((r) => r.id)).toEqual(["b"]);
  });
});
