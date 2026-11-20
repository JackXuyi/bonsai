"use client";

// 事实 / 偏好 / 边界（spec-m3 §2）：三段可折叠，只读。
// 边界区只展示——删除入口不在这里重复，编辑仍归工具页的边界清单。
import { formatWhen } from "@/lib/format";
import type { Boundary } from "@/lib/types";
import type { MemoryFactRow, MemoryPrefRow } from "@/lib/types";

interface FactsPrefsListProps {
  facts: MemoryFactRow[];
  preferences: MemoryPrefRow[];
  boundaries: Boundary[];
}

const CATEGORY_LABEL: Record<string, string> = {
  like: "喜欢",
  habit: "习惯",
  context: "情境",
  boundary: "边界",
};

function Group({
  title,
  sub,
  count,
  children,
  empty,
}: {
  title: string;
  sub: string;
  count: number;
  children: React.ReactNode;
  empty: string;
}) {
  return (
    <details className="mem-group" open>
      <summary>
        <span className="card-title">{title}</span>
        <span className="card-sub">
          {count} 条 · {sub}
        </span>
      </summary>
      {count === 0 ? <p className="mem-empty">{empty}</p> : children}
    </details>
  );
}

export function FactsPrefsList({ facts, preferences, boundaries }: FactsPrefsListProps) {
  return (
    <div className="card mem-groups">
      <Group
        title="事实"
        sub="夜间整理抽取"
        count={facts.length}
        empty="还没有抽取出事实。攒够几轮对话后自动整理。"
      >
        {facts.map((f) => (
          <div key={f.id} className="mem-row">
            <span className="mem-kind">fact</span>
            <span className="mem-text">{f.content}</span>
            <span className="mem-ts">
              置信 {f.confidence.toFixed(2)} · {formatWhen(f.ts)}
            </span>
          </div>
        ))}
      </Group>

      <Group
        title="偏好"
        sub="重复提到才会升级成偏好"
        count={preferences.length}
        empty="还没有偏好。"
      >
        {preferences.map((p) => (
          <div key={p.id} className="mem-row">
            <span className="mem-kind">{p.category}</span>
            <span className="mem-text">
              {CATEGORY_LABEL[p.category] ?? p.category} · {p.content}
            </span>
            <span className="mem-ts">
              置信 {p.confidence.toFixed(2)} · {formatWhen(p.ts)}
            </span>
          </div>
        ))}
      </Group>

      <Group
        title="边界"
        sub="最优先 · 只展示，改删去工具页"
        count={boundaries.length}
        empty="还没设过边界。onboarding 里可以补。"
      >
        {boundaries.map((b) => (
          <div key={b.id} className="mem-row boundary">
            <span className="mem-kind">boundary</span>
            <span className="mem-text">{b.content}</span>
          </div>
        ))}
      </Group>
    </div>
  );
}
