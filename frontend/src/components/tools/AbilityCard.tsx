"use client";

// 能力线卡（P4-4）：一行文字 + 重新练手按钮。不做进度条——分数永不外露。
import { useRef, useState } from "react";
import { abilityReset } from "@/lib/commands";
import { drainAbilityAnnouncements } from "@/lib/announcements";
import { toast, useUiStore } from "@/lib/stores/uiStore";

const ABILITY_LABELS = ["陪聊", "能办事", "真帮手"];

export function AbilityCard() {
  const ab = useUiStore((s) => s.ability);
  const [armed, setArmed] = useState(false);
  const armTimer = useRef<ReturnType<typeof setTimeout>>();

  function line(): string {
    if (!ab) return "它靠把事办成慢慢变能干";
    if (ab.stage === 0) {
      const left = 31 - (ab.day_index ?? 0);
      return left > 0
        ? `还在陪聊。大概还要 ${left} 天，看你让不让它办成几件事。`
        : "还在陪聊——办成的事还不够，升不了阶。";
    }
    if (ab.stage === 1) {
      return "能办事了。你给目标，它按步骤走完并校验。";
    }
    return "真帮手了。计划、校验、收尾，它按这个节奏来。";
  }

  function sub(): string {
    if (!ab) return "还在陪聊";
    const day = (ab.day_index ?? -1) >= 0 ? ` · 第 ${(ab.day_index ?? 0) + 1} 天` : "";
    return `${ab.stage_name || ABILITY_LABELS[ab.stage] || ""}${day}`;
  }

  async function reset() {
    if (!armed) {
      setArmed(true);
      clearTimeout(armTimer.current);
      armTimer.current = setTimeout(() => setArmed(false), 3000);
      return;
    }
    setArmed(false);
    await abilityReset().catch(() => {});
    await drainAbilityAnnouncements().catch(() => {});
    toast("能力线清零了。记忆、盆栽和关系都没动。");
  }

  return (
    <section className="card">
      <div className="card-head">
        <div className="card-titles">
          <p className="card-title">它办事的本事</p>
          <p className="card-sub">{sub()}</p>
        </div>
        <button
          className="ob-btn rel-reset"
          title="只清能力线，不动记忆、盆栽和关系"
          onClick={reset}
        >
          {armed ? "再点一次" : "重新练手"}
        </button>
      </div>
      <p className="status-line">{line()}</p>
    </section>
  );
}
