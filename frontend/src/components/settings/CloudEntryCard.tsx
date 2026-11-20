"use client";

// 云端配置卡（spec-m2 §3）：名称 / 类型 tag / 模型 / 端点 / 脱敏 key，
// 点卡片选中生效，右上角编辑 / 删除（删除由父级二次确认）。
import type { CloudEntryView } from "@/lib/types";

const KIND_LABEL: Record<CloudEntryView["kind"], string> = {
  custom: "自定义兼容",
  openai: "OpenAI 官方",
  claude: "Claude 官方",
};

interface CloudEntryCardProps {
  entry: CloudEntryView;
  active: boolean;
  testing: boolean;
  onPick: (id: string) => void;
  onEdit: (entry: CloudEntryView) => void;
  onRemove: (id: string) => void;
}

export function CloudEntryCard({
  entry,
  active,
  testing,
  onPick,
  onEdit,
  onRemove,
}: CloudEntryCardProps) {
  const cls = ["ccard", active ? "on" : "", testing ? "testing" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={cls}
      role="button"
      tabIndex={0}
      aria-pressed={active}
      title="点一下选中 · 右上角编辑 / 删除"
      onClick={() => !testing && onPick(entry.id)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          if (!testing) onPick(entry.id);
        }
      }}
    >
      <div className="ccard-head">
        <span className="ccard-name">{entry.name}</span>
        {testing ? (
          <span className="m-spin" aria-label="测试中" />
        ) : (
          <span className="ccard-actions">
            <button
              type="button"
              className="btn-ghost"
              aria-label={`编辑 ${entry.name}`}
              onClick={(e) => {
                e.stopPropagation();
                onEdit(entry);
              }}
            >
              编辑
            </button>
            <button
              type="button"
              className="btn-ghost"
              aria-label={`删除 ${entry.name}`}
              onClick={(e) => {
                e.stopPropagation();
                onRemove(entry.id);
              }}
            >
              ×
            </button>
          </span>
        )}
      </div>
      <div className="ccard-meta">
        {entry.model} · {entry.baseUrl.replace(/^https?:\/\//, "")}
      </div>
      <div className="ccard-tags">
        <span className="tag ram">{KIND_LABEL[entry.kind]}</span>
        <span className="tag ram">key {entry.maskedKey || "—"}</span>
        {active && <span className="tag ok">使用中</span>}
      </div>
    </div>
  );
}
