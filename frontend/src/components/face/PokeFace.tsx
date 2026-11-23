"use client";

// 可点的脸：poke 波纹动画 + facePoke 命令 + 浏览器预览的 greeted 演示
// （旧 Hero 的 onClick/poke 逻辑拆出——侧栏迷你脸与表情页大脸共用）。
import { useRef, useState } from "react";
import { POKE_LINES } from "./frames";
import { faceSkinById } from "./skins";
import { useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { facePoke } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";

interface PokeFaceProps {
  /** 尺寸类：sm=侧栏 52px / lg=表情页 150px（.face-sm/.face-lg） */
  size: "sm" | "lg";
  frame: number;
}

export function PokeFace({ size, frame }: PokeFaceProps) {
  const faceState = useUiStore((s) => s.faceState);
  const growth = useUiStore((s) => s.growth);
  const stagePreview = useUiStore((s) => s.stagePreview);
  const [poked, setPoked] = useState(false);
  const pokeTimer = useRef<ReturnType<typeof setTimeout>>();

  const stage = stagePreview?.stage ?? growth?.stage ?? 0;
  const dormant = stagePreview?.dormant ?? growth?.dormant ?? false;
  const skin = faceSkinById(useFaceSkinStore((s) => s.skinId));
  const Skin = skin.Component;

  function onClick() {
    setPoked(false);
    requestAnimationFrame(() => setPoked(true));
    clearTimeout(pokeTimer.current);
    pokeTimer.current = setTimeout(() => setPoked(false), 600);

    if (inTauri) {
      facePoke().catch(() => {});
      // Stage-flavoured poke copy (plan §3.7)
      useUiStore.getState().pushToast(
        POKE_LINES[Math.min(growth?.stage ?? 0, POKE_LINES.length - 1)]
      );
    } else {
      useUiStore.setState({ faceState: "greeted" });
      setTimeout(() => useUiStore.setState({ faceState: "calm" }), 2500);
    }
  }

  return (
    <button
      className={`face face-${size}${poked ? " poked" : ""}`}
      data-state={faceState}
      data-skin={skin.id}
      aria-label="和阿盆打招呼"
      title="点一下，打个招呼"
      onClick={onClick}
    >
      <span className="ripple" aria-hidden="true" />
      <Skin
        state={faceState}
        frame={frame}
        stage={stage}
        dormant={dormant}
        dayIndex={growth?.day_index}
        seed={growth?.micro_seed ?? 1}
      />
    </button>
  );
}
