"use client";

// 工具参数可读展示：授权条 / 审计 / 追踪共用。
import { parseToolArgLines } from "@/lib/formatToolArgs";

export function ToolArgsView({
  raw,
  className = "tool-args",
}: {
  raw: string;
  className?: string;
}) {
  const lines = parseToolArgLines(raw);
  if (lines.length === 0) return null;

  const allPlain = lines.every((l) => !l.key);
  if (allPlain) {
    return <pre className={`${className} tool-args-pre`}>{raw}</pre>;
  }

  return (
    <dl className={className} data-testid="tool-args">
      {lines.map((line, i) => (
        <div key={`${line.key ?? "v"}-${i}`} className="tool-args-row">
          {line.key ? <dt>{line.key}</dt> : null}
          <dd>{line.value}</dd>
        </div>
      ))}
    </dl>
  );
}
