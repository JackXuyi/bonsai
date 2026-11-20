"use client";

// 已装模型 grid（spec-m1 §3）：点卡片即切换；sidecar 不可达时展示静态
// fallback（目录 recommended 项，只读灰显——不冒充「已装」）。
import { ModelCard } from "./ModelCard";
import type { CatalogRow, OllamaModelRow } from "@/lib/types";

interface InstalledModelGridProps {
  installed: OllamaModelRow[];
  catalog: CatalogRow[];
  activeName: string;
  testingId: string | null;
  fallback: boolean;
  onPick: (tag: string) => void;
  onRemove: (tag: string) => void;
}

export function InstalledModelGrid({
  installed,
  catalog,
  activeName,
  testingId,
  fallback,
  onPick,
  onRemove,
}: InstalledModelGridProps) {
  const byTag = new Map(catalog.map((c) => [c.tag, c]));

  if (fallback) {
    const rows = catalog.filter((c) => c.recommended);
    return (
      <div className="mgrid">
        {rows.map((c) => (
          <ModelCard
            key={c.tag}
            row={{ name: c.tag, sizeBytes: c.sizeBytes, modified: "" }}
            meta={c}
            active={false}
            testing={false}
            disabled
            onPick={() => {}}
            onRemove={() => {}}
          />
        ))}
      </div>
    );
  }

  return (
    <div className="mgrid">
      {installed.map((row) => (
        <ModelCard
          key={row.name}
          row={row}
          meta={byTag.get(row.name)}
          active={row.name === activeName}
          testing={testingId === `ollama:${row.name}`}
          onPick={onPick}
          onRemove={onRemove}
        />
      ))}
    </div>
  );
}
