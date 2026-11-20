"use client";

// 最近事件（spec-m3 §2）：kind 等宽字体 + 派生事件灰显 + 相对时间。
// 派生事件（face_state / disagreement / relationship_*）不是用户说的话，
// 但确实在库里——如实展示、灰显标注，而不是藏掉。
import { formatWhen } from "@/lib/format";
import type { KindCount, MemoryEventRow } from "@/lib/types";

interface EventListProps {
  rows: MemoryEventRow[];
  /** 按 kind 分组的计数（徽章下方一行小字） */
  byKind: KindCount[];
}

export function EventList({ rows, byKind }: EventListProps) {
  return (
    <div className="card">
      <div className="card-head">
        <span className="card-title">最近事件</span>
        <span className="card-sub">时间倒序 · 派生事件灰显</span>
      </div>

      {byKind.length > 0 && (
        <div className="mem-kinds">
          {byKind.map((k) => (
            <span key={k.kind} className="mem-kind-chip">
              {k.kind} {k.count}
            </span>
          ))}
        </div>
      )}

      {rows.length === 0 ? (
        <p className="mem-empty">还没有事件。跟它说句话，这里就长东西了。</p>
      ) : (
        rows.map((r) => (
          <div
            key={r.id}
            className={`mem-row${r.derived ? " derived" : ""}`}
            data-derived={r.derived ? "1" : "0"}
          >
            <span className="mem-kind">{r.kind}</span>
            <span className="mem-text">{r.content}</span>
            <span className="mem-ts">{formatWhen(r.ts)}</span>
          </div>
        ))
      )}
    </div>
  );
}
