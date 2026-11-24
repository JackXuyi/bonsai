"use client";

/* eslint-disable @next/next/no-img-element -- 皮肤图片是 Tauri 本地/静态资源，
   尺寸固定，next/image 优化器在桌面 bundle 无收益（同 skins.tsx 豁免理由）。 */

// 可点的脸：poke 波纹动画 + facePoke 命令 + 浏览器预览的 greeted 演示
// （旧 Hero 的 onClick/poke 逻辑拆出——侧栏迷你脸与表情页大脸共用）。
// 渲染按皮肤 kind 分流（plan-face-custom-skins.md §3.2）：
// svg → Component 契约；image/custom → <img>（缺槽回退 FALLBACK_IMAGE）。
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { POKE_LINES } from "./frames";
import { FALLBACK_IMAGE, faceSkinById, resolveSkinImage } from "./skins";
import { ensureFaceSkinsLoaded, useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { facePoke } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";
import type { FaceState } from "@/lib/types";

interface PokeFaceProps {
  /** 尺寸类：xs=顶栏头像 28px / sm=52px / lg=预览 150px */
  size: "xs" | "sm" | "lg";
  frame: number;
}

export function PokeFace({ size, frame }: PokeFaceProps) {
  const faceState = useUiStore((s) => s.faceState);
  const growth = useUiStore((s) => s.growth);
  const stagePreview = useUiStore((s) => s.stagePreview);
  const skinId = useFaceSkinStore((s) => s.skinId);
  const customSkins = useFaceSkinStore((s) => s.customSkins);
  const baseDir = useFaceSkinStore((s) => s.baseDir);
  const [poked, setPoked] = useState(false);
  const pokeTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    ensureFaceSkinsLoaded();
  }, []);

  const stage = stagePreview?.stage ?? growth?.stage ?? 0;
  const dormant = stagePreview?.dormant ?? growth?.dormant ?? false;
  // 选中 id 失效（皮肤被删 / persist 残留）→ 兜底回内置默认
  const skin = faceSkinById(skinId, customSkins) ?? faceSkinById("deepseek")!;

  // custom 皮肤：manifest 文件名 → asset:// URL（baseDir 未就绪时回退内置图）
  const resolveCustom = (sid: string, file: string) =>
    baseDir ? convertFileSrc(`${baseDir}/face_skins/${sid}/${file}`) : FALLBACK_IMAGE;

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
      {skin.kind === "svg" ? (
        <skin.Component
          state={faceState}
          frame={frame}
          stage={stage}
          dormant={dormant}
          dayIndex={growth?.day_index}
          seed={growth?.micro_seed ?? 1}
        />
      ) : (
        <img
          src={resolveSkinImage(skin, faceState as FaceState, resolveCustom)?.src ?? FALLBACK_IMAGE}
          alt={skin.name}
          data-skin={skin.id}
          data-dormant={dormant ? "true" : "false"}
          data-frame={frame % 2}
          draggable={false}
        />
      )}
    </button>
  );
}
