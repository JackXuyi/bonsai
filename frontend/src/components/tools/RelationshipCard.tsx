"use client";

// 关系线卡（旧 relationship 段）：一行文字 + 重新认识按钮。
// 不做面板、不做进度条——有面板就会有人盯着进度，关系就变回数值游戏。
import { useRef, useState } from "react";
import { relationshipReset } from "@/lib/commands";
import { drainRelationshipAnnouncements } from "@/lib/announcements";
import { toast, useUiStore } from "@/lib/stores/uiStore";

const RELATIONSHIP_LABELS = ["拘谨", "熟络", "会顶嘴"];

export function RelationshipCard() {
  const rel = useUiStore((s) => s.relationship);
  const [armed, setArmed] = useState(false);
  const armTimer = useRef<ReturnType<typeof setTimeout>>();

  // 文案（旧 renderRelationship 原样）
  function line(): string {
    if (!rel) return "它靠猜对你的作息慢慢熟起来";
    if (rel.cold) return "最近有点生分，它说话收着点了。";
    if (rel.paused) return rel.pause_reason || "它先不往前凑了。";
    if (rel.stage === 0) {
      const left = 9 - (rel.day_index ?? 0);
      return left > 0
        ? `还在互相观察。大概还要 ${left} 天，看你让不让它猜。`
        : "还在互相观察——它想猜，但还没猜准过几次。";
    }
    if (rel.stage === 1) {
      return "它开始按你的作息说话了，偶尔会提你之前说过的事。";
    }
    return "现在它会直接说不同意，不绕弯子。";
  }

  function sub(): string {
    if (!rel) return "还在互相观察";
    const day = (rel.day_index ?? -1) >= 0 ? ` · 第 ${(rel.day_index ?? 0) + 1} 天` : "";
    return `${rel.stage_name || RELATIONSHIP_LABELS[rel.stage] || ""}${day}`;
  }

  async function reset() {
    // Two-step arm instead of a native confirm(): webview dialogs are not
    // guaranteed on every platform. Second click actually resets.
    if (!armed) {
      setArmed(true);
      clearTimeout(armTimer.current);
      armTimer.current = setTimeout(() => setArmed(false), 3000);
      return;
    }
    setArmed(false);
    await relationshipReset().catch(() => {});
    await drainRelationshipAnnouncements().catch(() => {});
    toast("重新认识了。记忆和盆栽都没动。");
  }

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">它对你的理解</p>
          <p className="card-sub">{sub()}</p>
        </div>
        <button
          className="ob-btn rel-reset"
          title="只清关系线，不动记忆和盆栽"
          onClick={reset}
        >
          {armed ? "再点一次" : "重新认识"}
        </button>
      </div>
      <p className="status-line">{line()}</p>
    </section>
  );
}
