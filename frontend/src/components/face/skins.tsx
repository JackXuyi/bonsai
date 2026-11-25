/* eslint-disable @next/next/no-img-element -- Tauri 桌面端本地静态资源，
   尺寸固定（侧栏 / 表情页），next/image 优化器在桌面 bundle 无收益。 */
// 脸部皮肤注册表（plan-presence-im v4）。渲染管线由 kind 区分：
// - image：单张静态图（官方小鲸兜底）；
// - image-set：内置表情槽（大肥鱼 / 猫鲸 CC0）；
// - custom：用户上传的 7 槽位图片集。
import type { FaceSkinManifest, FaceState } from "@/lib/types";
import { FACE_STATES } from "@/lib/types";

interface FaceSkinCommon {
  id: string;
  name: string;
  desc: string;
  /** 自定义皮肤（槽位可管理） */
  custom?: boolean;
}

export type FaceSkin =
  | (FaceSkinCommon & { kind: "image"; image: string })
  | (FaceSkinCommon & { kind: "image-set"; slots: Record<FaceState, string> })
  | (FaceSkinCommon & { kind: "custom"; manifest: FaceSkinManifest });

export const DEFAULT_SKIN = "fatfish";

export const FALLBACK_IMAGE = "/skins/deepseek-whale.png";

/** 已移除的内置 id → 回落默认（persist 迁移）。 */
export const REMOVED_BUILTIN_SKINS: Record<string, string> = {
  bonsai: DEFAULT_SKIN,
};

/** dsh-meme dafeiyu-001：白底已抠除为透明 PNG；calm=偷看。 */
const FATFISH_SLOTS: Record<FaceState, string> = {
  calm: "/skins/fatfish/calm.png",
  focused: "/skins/fatfish/focused.png",
  sleepy: "/skins/fatfish/sleepy.png",
  greeted: "/skins/fatfish/greeted.png",
  displeased: "/skins/fatfish/displeased.png",
  thinking: "/skins/fatfish/thinking.png",
  proud: "/skins/fatfish/proud.png",
};

/** 内置皮肤（只读，不允许覆盖上传）。 */
export const BUILTIN_SKINS: FaceSkin[] = [
  {
    kind: "image-set",
    id: "fatfish",
    name: "大肥鱼",
    desc: "dsh-meme 大肥鱼表情 · 非 DeepSeek 官方",
    slots: FATFISH_SLOTS,
  },
  {
    kind: "image",
    id: "deepseek",
    name: "小鲸 · DeepSeek",
    desc: "DeepSeek 官方蓝鲸标（兜底）",
    image: FALLBACK_IMAGE,
  },
];

/** 内置 + 自定义合并查找；都不命中返回 null（调用方兜底回默认）。 */
export function faceSkinById(
  id: string | undefined | null,
  customSkins: FaceSkinManifest[] = [],
): FaceSkin | null {
  const resolved = id && REMOVED_BUILTIN_SKINS[id] ? REMOVED_BUILTIN_SKINS[id] : id;
  const builtin = BUILTIN_SKINS.find((s) => s.id === resolved);
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
 * 解析某表情状态要渲染的图片 src。
 * - image-set / custom 缺槽回退 calm；再缺 → null（调用方用 FALLBACK_IMAGE）。
 */
export function resolveSkinImage(
  skin: FaceSkin,
  state: FaceState,
  resolveCustom?: (skinId: string, file: string) => string | undefined,
): SkinImage | null {
  if (skin.kind === "image") return { src: skin.image, custom: false };
  if (skin.kind === "image-set") {
    const src = skin.slots[state] ?? skin.slots.calm;
    return src ? { src, custom: false } : null;
  }
  if (skin.kind === "custom") {
    const file = skin.manifest.slots[state] ?? skin.manifest.slots["calm"];
    const src = file ? resolveCustom?.(skin.manifest.id, file) : undefined;
    if (!src) return null;
    return { src, custom: true };
  }
  return null;
}

/** 皮肤卡预览：取 calm 静帧。 */
export function SkinPreview({
  skin,
  resolveCustom,
}: {
  skin: FaceSkin;
  resolveCustom?: (skinId: string, file: string) => string | undefined;
}) {
  const img = resolveSkinImage(skin, "calm", resolveCustom);
  return <img src={img?.src ?? FALLBACK_IMAGE} alt={skin.name} draggable={false} />;
}

/** 开发期断言：image-set 槽位齐全。 */
export function assertFatfishSlotsComplete(): boolean {
  return FACE_STATES.every((s) => Boolean(FATFISH_SLOTS[s]));
}
