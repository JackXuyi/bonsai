"use client";

// 脸部皮肤选择（plan-face-custom-skins + plan-presence-im v4）。
// 选中 id 走 zustand persist；自定义列表运行时加载。
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { appDataDir } from "@tauri-apps/api/path";
import { faceSkinList } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { DEFAULT_SKIN, REMOVED_BUILTIN_SKINS } from "@/components/face/skins";
import type { FaceSkinManifest } from "@/lib/types";

interface FaceSkinState {
  skinId: string;
  customSkins: FaceSkinManifest[];
  /** appDataDir 基路径（custom 皮肤 asset:// URL 解析依赖），null = 未就绪 */
  baseDir: string | null;
  setSkin: (id: string) => void;
  refreshCustomSkins: () => Promise<void>;
}

function migrateSkinId(id: string | undefined): string {
  if (!id) return DEFAULT_SKIN;
  return REMOVED_BUILTIN_SKINS[id] ?? id;
}

export const useFaceSkinStore = create<FaceSkinState>()(
  persist(
    (set) => ({
      skinId: DEFAULT_SKIN,
      customSkins: [],
      baseDir: null,
      setSkin: (skinId) => set({ skinId: migrateSkinId(skinId) }),
      refreshCustomSkins: async () => {
        if (!inTauri) return;
        try {
          const [skins, dir] = await Promise.all([faceSkinList(), appDataDir()]);
          set({ customSkins: skins, baseDir: dir });
        } catch {
          // 加载失败不阻塞：customSkins 空、baseDir null，PokeFace 走 FALLBACK_IMAGE。
        }
      },
    }),
    {
      name: "bonsai-face-skin",
      partialize: (s) => ({ skinId: s.skinId }),
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<FaceSkinState>;
        return {
          ...current,
          ...p,
          skinId: migrateSkinId(p.skinId ?? current.skinId),
        };
      },
    },
  ),
);

/** 应用启动后懒加载一次（PokeFace / FaceSkinCard 挂载时调用）。 */
let skinsLoaded = false;
export function ensureFaceSkinsLoaded() {
  if (skinsLoaded) return;
  skinsLoaded = true;
  // 冷启再跑一次迁移（persist rehydrate 后）
  const id = useFaceSkinStore.getState().skinId;
  const next = migrateSkinId(id);
  if (next !== id) useFaceSkinStore.getState().setSkin(next);
  void useFaceSkinStore.getState().refreshCustomSkins();
}
