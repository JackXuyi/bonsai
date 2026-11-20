"use client";

// 工具页 · 人格卡（spec-m4 §4A）：只读合成视图——宪法要点（折叠）、
// 表达规范、当前语气档、表情与形态阶段。换模型只换表达，不换本体；
// 人格编辑器是 plan 明确不做的东西（plan §5.5）。
import { useCallback, useEffect, useState } from "react";
import { personaSnapshot } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useTauriEvent } from "@/lib/tauriListen";
import type { PersonaSnapshot, RelationshipMode } from "@/lib/types";

/** 语气档中文映射（spec-m4 §4A，含 rusty）。 */
export const MODE_LABEL: Record<RelationshipMode, string> = {
  reserved: "拘谨",
  familiar: "熟络",
  direct: "会顶嘴",
  rusty: "生疏",
};

const GROWTH_NAMES = ["破土", "抽枝", "成形", "成景"];

const FACE_LABEL: Record<string, string> = {
  calm: "平静",
  focused: "专注",
  sleepy: "犯困",
  greeted: "打过招呼",
  displeased: "不高兴",
  thinking: "在想",
  proud: "得意",
};

export function PersonaCard() {
  const [snap, setSnap] = useState<PersonaSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!inTauri) return; // 浏览器预览：安静空态，不编数据
    try {
      setSnap(await personaSnapshot());
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 三个状态机任何一个动了 → 重拉快照（spec-m4 §4A）
  useTauriEvent<unknown>("relationship:stage", () => void refresh());
  useTauriEvent<unknown>("growth:stage", () => void refresh());
  useTauriEvent<unknown>("face:state", () => void refresh());

  if (error) {
    return (
      <div className="card">
        <div className="card-head">
          <span className="card-title">人格</span>
        </div>
        <p className="status-line">读不到人格快照：{error}</p>
      </div>
    );
  }

  if (!snap) {
    return null;
  }

  const growthName = GROWTH_NAMES[snap.growthStage] ?? "破土";
  const faceName = FACE_LABEL[snap.faceState] ?? snap.faceState;

  return (
    <div className="card" data-testid="persona-card">
      <div className="card-head">
        <span className="card-title">人格</span>
        <span className="card-sub">只读 · 换模型不换本体</span>
      </div>

      <details className="persona-fold">
        <summary>
          宪法要点<span className="persona-count">{snap.card.constitutionItems.length} 条</span>
        </summary>
        <ul className="persona-items">
          {snap.card.constitutionItems.map((item, i) => (
            <li key={i}>{item}</li>
          ))}
        </ul>
      </details>

      {snap.card.bannedForms.length > 0 && (
        <details className="persona-fold">
          <summary>
            表达规范
            <span className="persona-count">{snap.card.bannedForms.length} 条</span>
          </summary>
          <ul className="persona-items">
            {snap.card.bannedForms.map((item, i) => (
              <li key={i}>不说「{item}」</li>
            ))}
          </ul>
        </details>
      )}

      <div className="persona-now">
        <span>
          当前语气：<strong>{MODE_LABEL[snap.relationshipMode]}</strong>
        </span>
        <span>
          表情：<strong>{faceName}</strong>
          {snap.dormant && <em className="persona-dormant">（休眠中）</em>}
        </span>
        <span>
          形态：<strong>{growthName}</strong>
        </span>
      </div>

      <p className="hint-line">换模型只换表达，不换本体。</p>
    </div>
  );
}
