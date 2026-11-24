// 权限档徽标：只读 / 询问 / 禁止（roadmap G-10）。
import {
  TOOL_ACCESS_HINT,
  TOOL_ACCESS_LABEL,
  normalizeToolAccess,
  type ToolAccess,
} from "@/lib/toolAccess";

export interface ToolAccessBadgeProps {
  access?: string | null;
  /** 紧凑：只显示标签，不写 title 外的提示。 */
  compact?: boolean;
}

export function ToolAccessBadge({ access, compact }: ToolAccessBadgeProps) {
  const tier: ToolAccess = normalizeToolAccess(access);
  return (
    <span
      className="agent-access"
      data-access={tier}
      data-testid="tool-access-badge"
      title={TOOL_ACCESS_HINT[tier]}
    >
      {TOOL_ACCESS_LABEL[tier]}
      {!compact && <span className="sr-only">：{TOOL_ACCESS_HINT[tier]}</span>}
    </span>
  );
}
