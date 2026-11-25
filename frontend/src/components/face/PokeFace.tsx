"use client";

/* eslint-disable @next/next/no-img-element -- 皮肤图片是 Tauri 本地/静态资源，
   尺寸固定，next/image 优化器在桌面 bundle 无收益（同 skins.tsx 豁免理由）。 */

// 可点的脸：poke 波纹 + facePoke；image / image-set / custom 均走 <img>。
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { POKE_LINES, MOODS } from "./frames";
import { DEFAULT_SKIN, FALLBACK_IMAGE, faceSkinById, resolveSkinImage } from "./skins";
import { ensureFaceSkinsLoaded, useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import { usePersonaStore } from "@/lib/stores/personaStore";
import { facePoke } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";
import type { FaceState } from "@/lib/types";

interface PokeFaceProps {
  /** 尺寸类：xs=顶栏 28px / sm=52px / md=ChatPet / lg=预览 150px */
  size: "xs" | "sm" | "md" | "lg";
  frame: number;
  /** 覆盖全局 faceState（如 ChatPet 流式中强制 thinking） */
  stateOverride?: FaceState;
  /** 额外 class（ChatPet 定位用） */
  className?: string;
  title?: string;
}

export function PokeFace({ size, frame, stateOverride, className, title }: PokeFaceProps) {
  const faceState = useUiStore((s) => s.faceState);
  const growth = useUiStore((s) => s.growth);
  const stagePreview = useUiStore((s) => s.stagePreview);
  const skinId = useFaceSkinStore((s) => s.skinId);
  const customSkins = useFaceSkinStore((s) => s.customSkins);
  const baseDir = useFaceSkinStore((s) => s.baseDir);
  const personaName = usePersonaStore((s) => s.name);
  const ensurePersona = usePersonaStore((s) => s.ensureLoaded);
  const [poked, setPoked] = useState(false);
  const pokeTimer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    ensureFaceSkinsLoaded();
    ensurePersona();
  }, [ensurePersona]);

  const dormant = stagePreview?.dormant ?? growth?.dormant ?? false;
  const skin = faceSkinById(skinId, customSkins) ?? faceSkinById(DEFAULT_SKIN)!;
  const shownState = stateOverride ?? faceState;

  const resolveCustom = (sid: string, file: string) =>
    baseDir ? convertFileSrc(`${baseDir}/face_skins/${sid}/${file}`) : FALLBACK_IMAGE;

  function onClick() {
    setPoked(false);
    requestAnimationFrame(() => setPoked(true));
    clearTimeout(pokeTimer.current);
    pokeTimer.current = setTimeout(() => setPoked(false), 600);

    if (inTauri) {
      facePoke().catch(() => {});
      useUiStore.getState().pushToast(
        POKE_LINES[Math.min(growth?.stage ?? 0, POKE_LINES.length - 1)],
      );
    } else {
      useUiStore.setState({ faceState: "greeted" });
      setTimeout(() => useUiStore.setState({ faceState: "calm" }), 2500);
    }
  }

  const mood = MOODS[shownState] ?? MOODS.calm;
  const tip = title ?? `${personaName} · ${mood}`;

  return (
    <button
      className={`face face-${size}${poked ? " poked" : ""}${className ? " " + className : ""}`}
      data-state={shownState}
      data-skin={skin.id}
      aria-label={`和${personaName}打招呼`}
      title={tip}
      onClick={onClick}
    >
      <span className="ripple" aria-hidden="true" />
      <img
        src={resolveSkinImage(skin, shownState as FaceState, resolveCustom)?.src ?? FALLBACK_IMAGE}
        alt={skin.name}
        data-skin={skin.id}
        data-dormant={dormant ? "true" : "false"}
        data-frame={frame % 2}
        draggable={false}
      />
    </button>
  );
}
