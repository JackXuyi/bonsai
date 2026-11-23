"use client";

// 脸部皮肤选择（纯前端外观偏好，zustand persist 落 localStorage）。
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { DEFAULT_SKIN, type FaceSkinId } from "@/components/face/skins";

interface FaceSkinState {
  skinId: FaceSkinId;
  setSkin: (id: FaceSkinId) => void;
}

export const useFaceSkinStore = create<FaceSkinState>()(
  persist(
    (set) => ({
      skinId: DEFAULT_SKIN,
      setSkin: (skinId) => set({ skinId }),
    }),
    { name: "bonsai-face-skin" },
  ),
);
