/* eslint-disable @next/next/no-img-element -- Tauri 桌面端本地静态资源，
   尺寸固定（侧栏 52px / 表情页 150px），next/image 优化器在桌面 bundle 无收益。 */
// 脸部皮肤注册表（plan-face-custom-skins.md §6.1）。三类皮肤共享同一渲染
// 出口（PokeFace），渲染管线由 kind 区分：
// - svg：内置盆栽经典，Component 契约画 200×200 画布（成长形态线生效）；
// - image：内置 DeepSeek 小鲸，单张静态图；
// - custom：用户上传的 7 槽位图片集（face_skin_list 的 manifest），
//   渲染优先级 槽位图 > calm 必传图 > FALLBACK_IMAGE。
// 图片素材来源：frontend/public/skins/（deepseek-whale.png 来自
// Wikimedia Commons 的 DeepSeek 官方鲸鱼 logo，2026-11-23 下载）。
import type { ComponentType } from "react";
import { BonsaiFace } from "./BonsaiFace";
import type { FaceSkinManifest, FaceState } from "@/lib/types";

export interface FaceSkinProps {
  state: FaceState;
  /** 帧序号（0/1，父级 600ms 翻转驱动动画） */
  frame: number;
  stage: number;
  dormant: boolean;
  dayIndex?: number;
  seed?: number;
}

interface FaceSkinCommon {
  id: string;
  name: string;
  desc: string;
  /** 自定义皮肤（槽位可管理） */
  custom?: boolean;
}

export type FaceSkin =
  | (FaceSkinCommon & { kind: "svg"; Component: ComponentType<FaceSkinProps> })
  | (FaceSkinCommon & { kind: "image"; image: string })
  | (FaceSkinCommon & { kind: "custom"; manifest: FaceSkinManifest });

export const DEFAULT_SKIN = "deepseek";

export const FALLBACK_IMAGE = "/skins/deepseek-whale.png";

/** 内置皮肤（只读，不允许覆盖上传）。 */
export const BUILTIN_SKINS: FaceSkin[] = [
  {
    kind: "image",
    id: "deepseek",
    name: "小鲸 · DeepSeek",
    desc: "DeepSeek 的蓝色小鲸形象",
    image: FALLBACK_IMAGE,
  },
  {
    kind: "svg",
    id: "bonsai",
    name: "盆栽 · 经典",
    desc: "老朋友阿盆，窗台上的那棵小树",
    Component: BonsaiFace,
  },
];

/** 内置 + 自定义合并查找；都不命中返回 null（调用方兜底回默认）。 */
export function faceSkinById(id: string | undefined | null, customSkins: FaceSkinManifest[] = []): FaceSkin | null {
  const builtin = BUILTIN_SKINS.find((s) => s.id === id);
  if (builtin) return builtin;
  const m = customSkins.find((c) => c.id === id);
  if (m) {
    return { kind: "custom", id: m.id, name: m.name, desc: "自定义皮肤", custom: true, manifest: m };
  }
  return null;
}

export interface SkinImage {
  src: string;
  /** 自定义皮肤（src 依赖 baseDir 解析，可能未就绪） */
  custom: boolean;
}

/**
 * 解析某表情状态要渲染的图片 src（image / custom 管线）。
 * - custom 缺槽回退 calm；calm 也没传 → null（调用方用 FALLBACK_IMAGE）。
 * - resolveCustom：把 (skinId, file) 翻译成可访问 URL（asset protocol），
 *   未就绪时传 undefined → 返回 null。
 */
export function resolveSkinImage(
  skin: FaceSkin,
  state: FaceState,
  resolveCustom?: (skinId: string, file: string) => string | undefined,
): SkinImage | null {
  if (skin.kind === "image") return { src: skin.image, custom: false };
  if (skin.kind === "custom") {
    const file = skin.manifest.slots[state] ?? skin.manifest.slots["calm"];
    const src = file ? resolveCustom?.(skin.manifest.id, file) : undefined;
    if (!src) return null;
    return { src, custom: true };
  }
  return null;
}

/** 皮肤卡预览：image/custom → 静态图，svg → stage 2 成形态静帧。 */
export function SkinPreview({
  skin,
  resolveCustom,
}: {
  skin: FaceSkin;
  resolveCustom?: (skinId: string, file: string) => string | undefined;
}) {
  if (skin.kind === "svg") {
    const Component = skin.Component;
    return <Component state="calm" frame={0} stage={2} dormant={false} seed={1} />;
  }
  const img = resolveSkinImage(skin, "calm", resolveCustom);
  return <img src={img?.src ?? FALLBACK_IMAGE} alt={skin.name} draggable={false} />;
}
