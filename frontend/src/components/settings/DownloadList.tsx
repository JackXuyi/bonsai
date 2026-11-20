"use client";

// 可下载模型区（spec-m1 §3）：进度条 / 取消 / 完成后由订阅侧移入已装。
// think_ok=false 的模型默认折叠进「更多」（qwen3:4b 思维链污染正文的教训）。
import { useState } from "react";
import { formatBytes } from "@/lib/format";
import type { CatalogRow, PullProgress } from "@/lib/types";

interface DownloadListProps {
  catalog: CatalogRow[];
  installedNames: string[];
  pulls: Record<string, PullProgress>;
  onDownload: (tag: string) => void;
  onCancel: () => void;
}

function DownloadRow({
  row,
  pull,
  onDownload,
  onCancel,
}: {
  row: CatalogRow;
  pull?: PullProgress;
  onDownload: (tag: string) => void;
  onCancel: () => void;
}) {
  const active = pull && (pull.status === "pulling" || pull.status === "warn_disk");
  const percent = pull?.percent ?? 0;

  return (
    <div className={`dl-row${active ? " running" : ""}`}>
      <div className="dl-info">
        <p className="dl-name">
          {row.tag}
          {row.thinkOk ? null : <span className="tag warn">⚠ 思维链不可关</span>}
          {row.ramHint && <span className="tag ram">{row.ramHint}</span>}
        </p>
        <p className="dl-meta">
          {row.sizeLabel} · {row.note}
          {pull?.status === "warn_disk" && " · 磁盘可能不够"}
        </p>
        {active && (
          <div
            className="dl-bar"
            role="progressbar"
            aria-valuenow={percent}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <i style={{ width: `${percent}%` }} />
          </div>
        )}
      </div>
      {active ? (
        <div className="dl-actions">
          <span className="dl-pct">
            {formatBytes(pull!.completed)} / {pull!.total > 0 ? formatBytes(pull!.total) : "…"}
          </span>
          <button type="button" className="btn-ghost" onClick={onCancel}>
            取消
          </button>
        </div>
      ) : (
        <button type="button" className="btn" onClick={() => onDownload(row.tag)}>
          下载
        </button>
      )}
    </div>
  );
}

export function DownloadList({
  catalog,
  installedNames,
  pulls,
  onDownload,
  onCancel,
}: DownloadListProps) {
  const [showMore, setShowMore] = useState(false);

  const candidates = catalog.filter(
    (c) => !installedNames.some((n) => n === c.tag || n === `${c.tag}:latest`)
  );
  const primary = candidates.filter((c) => c.recommended);
  const more = candidates.filter((c) => !c.recommended);
  const busy = Object.values(pulls).some(
    (p) => p.status === "pulling" || p.status === "warn_disk"
  );

  if (candidates.length === 0) {
    return <p className="hint-line">推荐目录里的模型都装好了。</p>;
  }

  return (
    <div id="dl-list">
      {primary.map((row) => (
        <DownloadRow
          key={row.tag}
          row={row}
          pull={pulls[row.tag]}
          onDownload={busy ? () => {} : onDownload}
          onCancel={onCancel}
        />
      ))}
      {more.length > 0 && (
        <>
          <button
            type="button"
            className="more-toggle"
            aria-expanded={showMore}
            onClick={() => setShowMore((v) => !v)}
          >
            {showMore ? "▴ 收起" : `▾ 更多（${more.length}，含不推荐的模型）`}
          </button>
          {showMore &&
            more.map((row) => (
              <DownloadRow
                key={row.tag}
                row={row}
                pull={pulls[row.tag]}
                onDownload={busy ? () => {} : onDownload}
                onCancel={onCancel}
              />
            ))}
        </>
      )}
    </div>
  );
}
