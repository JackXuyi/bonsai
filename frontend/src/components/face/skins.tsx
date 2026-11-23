/* eslint-disable @next/next/no-img-element -- Tauri 桌面端本地静态资源，
   尺寸固定（侧栏 52px / 表情页 150px），next/image 优化器在桌面 bundle 无收益。 */
// 脸部皮肤注册表。两类皮肤共享同一渲染出口（PokeFace）：
// - svg 皮肤：Component 实现 FaceSkinProps，画 200×200 画布（盆栽经典）。
// - 图片皮肤：makeImageSkin 生成 <img> 组件，外观是静态形象图，
//   dormant 走 CSS 灰度降饱和，frame 驱动轻浮动（见 face.css）。
// 图片素材来源：frontend/public/skins/（deepseek-whale.png 来自
// Wikimedia Commons 的 DeepSeek 官方鲸鱼 logo，2026-11-23 下载）。
import type { ComponentType } from "react";
import { BonsaiFace } from "./BonsaiFace";
import type { FaceState } from "@/lib/types";

export type FaceSkinId = "deepseek" | "bonsai";

export interface FaceSkinProps {
  state: FaceState;
  /** 帧序号（0/1，父级 600ms 翻转驱动动画） */
  frame: number;
  stage: number;
  dormant: boolean;
  dayIndex?: number;
  seed?: number;
}

export interface FaceSkin {
  id: FaceSkinId;
  name: string;
  desc: string;
  Component: ComponentType<FaceSkinProps>;
}

/** 图片皮肤工厂：静态形象图 + dormant 灰度 + 帧轻浮动。 */
function makeImageSkin(skinId: FaceSkinId, image: string, alt: string) {
  function ImageSkinImpl({ frame, dormant }: FaceSkinProps) {
    return (
      <img
        src={image}
        alt={alt}
        data-skin={skinId}
        data-dormant={dormant ? "true" : "false"}
        data-frame={frame % 2}
        draggable={false}
      />
    );
  }
  return ImageSkinImpl;
}

/** 默认皮肤：DeepSeek 小鲸（2026-11-23 用户裁决）。 */
export const DEFAULT_SKIN: FaceSkinId = "deepseek";

export const FACE_SKINS: FaceSkin[] = [
  {
    id: "deepseek",
    name: "小鲸 · DeepSeek",
    desc: "DeepSeek 的蓝色小鲸形象",
    Component: makeImageSkin("deepseek", "/skins/deepseek-whale.png", "小鲸"),
  },
  {
    id: "bonsai",
    name: "盆栽 · 经典",
    desc: "老朋友阿盆，窗台上的那棵小树",
    Component: BonsaiFace,
  },
];

export function faceSkinById(id: string | undefined | null): FaceSkin {
  return FACE_SKINS.find((s) => s.id === id) ?? FACE_SKINS[0];
}
