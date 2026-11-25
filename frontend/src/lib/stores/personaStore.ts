"use client";

// 人格显示名缓存（权威在 Rust persona；写入经 set_persona_name）。
import { create } from "zustand";
import { getPersona, setPersonaName } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";

export const DEFAULT_PERSONA_NAME = "大肥鱼";

interface PersonaUiState {
  name: string;
  loaded: boolean;
  error: string | null;
  ensureLoaded: () => void;
  setName: (name: string) => Promise<void>;
}

let loadStarted = false;

export const usePersonaStore = create<PersonaUiState>((set, get) => ({
  name: DEFAULT_PERSONA_NAME,
  loaded: false,
  error: null,
  ensureLoaded: () => {
    if (loadStarted) return;
    loadStarted = true;
    if (!inTauri) {
      set({ loaded: true, name: DEFAULT_PERSONA_NAME });
      return;
    }
    try {
      void getPersona()
        .then((p) => {
          const name =
            typeof p?.name === "string" && p.name.trim() ? p.name.trim() : DEFAULT_PERSONA_NAME;
          set({ name, loaded: true, error: null });
        })
        .catch(() => {
          set({ loaded: true, name: DEFAULT_PERSONA_NAME });
        });
    } catch {
      set({ loaded: true, name: DEFAULT_PERSONA_NAME });
    }
  },
  setName: async (raw) => {
    const name = raw.trim();
    if (!inTauri) {
      set({ name: name || DEFAULT_PERSONA_NAME, error: null });
      return;
    }
    try {
      const updated = await setPersonaName(name);
      const next =
        typeof updated.name === "string" && updated.name.trim()
          ? updated.name.trim()
          : get().name;
      set({ name: next, error: null });
    } catch (e) {
      const msg = String(e);
      const idx = msg.indexOf(":");
      set({ error: idx > 0 ? msg.slice(idx + 1) : msg });
      throw e;
    }
  },
}));
