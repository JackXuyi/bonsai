"use client";

// 已装模型卡（spec-m1 §3）：点卡片即切换（无保存按钮）；右键删除（父级确认）。
// 三态：on（使用中 ✓）/ testing（spinner）/ 普通。
import { formatBytes, relativeDay } from "@/lib/format";
import type { CatalogRow, OllamaModelRow } from "@/lib/types";

interface ModelCardProps {
  row: OllamaModelRow;
  meta?: CatalogRow;
  active: boolean;
  testing: boolean;
  disabled?: boolean;
  onPick: (tag: string) => void;
  onRemove: (tag: string) => void;
}

export function ModelCard({
  row,
  meta,
  active,
  testing,
  disabled,
  onPick,
  onRemove,
}: ModelCardProps) {
  const cls = [
    "mcard",
    active ? "on" : "",
    testing ? "testing" : "",
    disabled ? "ghost" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <button
      type="button"
      className={cls}
      disabled={disabled || testing}
      aria-pressed={active}
      onClick={() => onPick(row.name)}
      onContextMenu={(e) => {
        e.preventDefault();
        onRemove(row.name);
      }}
      title="点一下切换 · 右键删除"
    >
      <span className="m-check" aria-hidden="true">
        ✓
      </span>
      {testing && <span className="m-spin" aria-label="测试中" />}
      <span className="m-name">{row.name}</span>
      <span className="m-meta">
        {formatBytes(row.sizeBytes)}
        {relativeDay(row.modified) ? ` · ${relativeDay(row.modified)}` : ""}
        {meta ? ` · ${meta.note}` : ""}
      </span>
      <span className="m-tags">
        {active && <span className="tag ok">使用中</span>}
        {meta?.thinkOk === false && <span className="tag warn">⚠ 思维链不可关</span>}
        {meta?.ramHint && <span className="tag ram">{meta.ramHint}</span>}
      </span>
    </button>
  );
}
