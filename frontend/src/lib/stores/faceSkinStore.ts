"use client";

// 脸部皮肤选择（plan-face-custom-skins.md §6.1）。选中 id 走 zustand
// persist 落 localStorage；自定义皮肤列表与 appDataDir 基路径运行时加载
// （face_skin_list 命令），不持久化——每次启动重新拉取。
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { appDataDir } from "@tauri-apps/api/path";
import { faceSkinList } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { DEFAULT_SKIN } from "@/components/face/skins";
import type { FaceSkinManifest } from "@/lib/types";

interface FaceSkinState {
  skinId: string;
  customSkins: FaceSkinManifest[];
  /** appDataDir 基路径（custom 皮肤 asset:// URL 解析依赖），null = 未就绪 */
  baseDir: string | null;
  setSkin: (id: string) => void;
  refreshCustomSkins: () => Promise<void>;
}

export const useFaceSkinStore = create<FaceSkinState>()(
  persist(
    (set) => ({
      skinId: DEFAULT_SKIN,
      customSkins: [],
      baseDir: null,
      setSkin: (skinId) => set({ skinId }),
      refreshCustomSkins: async () => {
        if (!inTauri) return;
        try {
          const [skins, dir] = await Promise.all([faceSkinList(), appDataDir()]);
          set({ customSkins: skins, baseDir: dir });
        } catch {
          // 加载失败不阻塞主流程：customSkins 保持空、baseDir null，
          // PokeFace 渲染走 FALLBACK_IMAGE 兜底。
        }
      },
    }),
    {
      name: "bonsai-face-skin",
      // 只持久化选中 id；列表与路径是运行时状态
      partialize: (s) => ({ skinId: s.skinId }),
    },
  ),
);

/** 应用启动后懒加载一次（PokeFace / FaceSkinCard 挂载时调用）。 */
let skinsLoaded = false;
export function ensureFaceSkinsLoaded() {
  if (skinsLoaded) return;
  skinsLoaded = true;
  void useFaceSkinStore.getState().refreshCustomSkins();
}
