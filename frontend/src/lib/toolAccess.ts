// 工具权限三档（plan-task-card §6 / roadmap G-10）：只读直行 / 询问 / 禁止。
// 档位由引擎在 ToolDescriptor 上声明，前端只展示，不可改写。

export type ToolAccess = "read" | "ask" | "forbidden";

export const TOOL_ACCESS_ORDER: ToolAccess[] = ["read", "ask", "forbidden"];

export const TOOL_ACCESS_LABEL: Record<ToolAccess, string> = {
  read: "只读",
  ask: "询问",
  forbidden: "禁止",
};

export const TOOL_ACCESS_HINT: Record<ToolAccess, string> = {
  read: "直接执行，记入审计",
  ask: "每次弹确认卡（写类强制，无视下方「自动放行」）",
  forbidden: "引擎直接拒绝，不执行",
};

/** 容忍缺省 / 未知值 → read（对齐 Rust serde default）。 */
export function normalizeToolAccess(raw: string | null | undefined): ToolAccess {
  if (raw === "ask" || raw === "forbidden" || raw === "read") return raw;
  return "read";
}
