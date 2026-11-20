"use client";

// 模型配置状态（spec-m1 本地 / spec-m2 云端）：已装列表 / 推荐目录 /
// 下载进度 / 本地切换 / 云端清单 CRUD / 云端切换。
// 命令失败信息统一 humanize 成一句话（错误码见 llm_admin.rs / llm_cloud.rs）。
import { create } from "zustand";
import {
  llmCatalog,
  llmCloudEntriesGet,
  llmCloudEntryRemove,
  llmCloudEntrySave,
  llmCloudTest,
  llmConfigGet,
  llmConfigSetActive,
  llmModelCancel,
  llmModelDownload,
  llmModelListInstalled,
  llmModelRemove,
} from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "./uiStore";
import type {
  CatalogRow,
  CloudEntryInput,
  CloudEntryView,
  CloudTestReport,
  LlmProvider,
  OllamaModelRow,
  PullProgress,
} from "@/lib/types";

/** 设置页顶部段选：本地 / 云端。 */
export type ModelTab = "local" | "cloud";

// 浏览器预览的演示数据（同 onboarding 的 demo-only fallback，非第二数据源）
const PREVIEW_INSTALLED: OllamaModelRow[] = [
  { name: "qwen3.5:2b", sizeBytes: 800_000_000, modified: "2026-11-16T10:00:00Z" },
  { name: "qwen3:4b", sizeBytes: 2_400_000_000, modified: "2026-11-15T10:00:00Z" },
];
const PREVIEW_CATALOG: CatalogRow[] = [
  { tag: "gemma3:4b", sizeLabel: "3.3GB", sizeBytes: 3_300_000_000, note: "Google · 中英均可", thinkOk: true, ramHint: "16GB+", recommended: true },
  { tag: "gemma3:1b", sizeLabel: "0.8GB", sizeBytes: 800_000_000, note: "Google · 轻量兜底", thinkOk: true, recommended: false },
  { tag: "llama3.2:3b", sizeLabel: "2.0GB", sizeBytes: 2_000_000_000, note: "Meta · 中英混合稍弱", thinkOk: true, recommended: false },
  { tag: "llama3.1:8b", sizeLabel: "4.9GB", sizeBytes: 4_900_000_000, note: "Meta · 8B 质量档", thinkOk: true, ramHint: "16GB+", recommended: false },
  { tag: "qwen3:8b", sizeLabel: "5.2GB", sizeBytes: 5_200_000_000, note: "⚠ 思维链不可关 · qwen3:4b 同款问题", thinkOk: false, ramHint: "16GB+", recommended: false },
  { tag: "deepseek-r1:1.5b", sizeLabel: "1.1GB", sizeBytes: 1_100_000_000, note: "⚠ 思维链不可关 · 会污染正文", thinkOk: false, recommended: false },
  { tag: "deepseek-r1:8b", sizeLabel: "4.9GB", sizeBytes: 4_900_000_000, note: "DeepSeek · 思维链不可关 · 会污染正文", thinkOk: false, ramHint: "16GB+", recommended: false },
  { tag: "deepseek-r1:14b", sizeLabel: "9.0GB", sizeBytes: 9_000_000_000, note: "DeepSeek · 蒸馏 14B · 思维链不可关", thinkOk: false, ramHint: "32GB+", recommended: false },
];
const PREVIEW_CLOUD: CloudEntryView[] = [
  { id: "demo-ds", name: "DeepSeek", kind: "custom", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-chat", maskedKey: "••••9f3a" },
];

/** 错误码 → 用户能懂的一句话。 */
export function humanizeLlmError(err: unknown): string {
  const s = String(err);
  if (s.includes("sidecar_down")) return "本地服务没起来（日志里看 sidecar）";
  if (s.includes("model_not_installed")) return "这个模型没装";
  if (s.includes("model_in_use")) return "先切换到别的模型，再删它";
  if (s.includes("download_busy")) return "已经有一个下载在跑了";
  if (s.includes("probe_failed")) return "模型加载失败，先别切";
  if (s.includes("test_failed")) return "没连上：检查接口地址 / Key / 模型名";
  if (s.includes("entry_not_found")) return "这条配置不在了，刷新一下";
  if (s.includes("unknown_id")) return "配置格式不对";
  return s;
}

const TERMINAL: PullProgress["status"][] = ["done", "error", "cancelled"];

interface LlmState {
  loaded: boolean;
  provider: LlmProvider;
  activeId: string;
  localModel: string;
  sidecarUp: boolean;
  installed: OllamaModelRow[];
  catalog: CatalogRow[];
  /** tag → 最近一次进度快照（终态由订阅方清理） */
  pulls: Record<string, PullProgress>;
  /** 正在测试切换的 id（`ollama:<tag>` / `cloud:<id>`），null = 空闲 */
  testingId: string | null;
  lastError: string | null;
  /** 设置页当前段选（切走再回来保持） */
  modelTab: ModelTab;
  cloudEntries: CloudEntryView[];

  load: () => Promise<void>;
  refreshInstalled: () => Promise<void>;
  setModelTab: (tab: ModelTab) => void;
  loadCloud: () => Promise<void>;
  /** 本地 / 云端共用的切换链路（`ollama:<tag>` / `cloud:<id>`）。 */
  activateId: (id: string) => Promise<boolean>;
  activate: (tag: string) => Promise<boolean>;
  activateCloud: (entryId: string) => Promise<boolean>;
  saveCloudEntry: (input: CloudEntryInput) => Promise<boolean>;
  removeCloudEntry: (entryId: string) => Promise<void>;
  testCloudEntry: (input: CloudEntryInput) => Promise<CloudTestReport | null>;
  download: (tag: string) => Promise<void>;
  cancel: () => Promise<void>;
  remove: (tag: string) => Promise<void>;
  applyPull: (p: PullProgress) => void;
  clearPull: (tag: string) => void;
}

export const useLlmStore = create<LlmState>((set, get) => ({
  loaded: false,
  provider: "ollama",
  activeId: "ollama:qwen3.5:2b",
  localModel: "qwen3.5:2b",
  sidecarUp: false,
  installed: [],
  catalog: [],
  pulls: {},
  testingId: null,
  lastError: null,
  modelTab: "local",
  cloudEntries: [],

  load: async () => {
    // 壳层与设置页都会触发；只跑一次（设置页的 refreshInstalled 负责增量）。
    if (get().loaded) return;
    if (!inTauri) {
      set({
        loaded: true,
        provider: "ollama",
        activeId: "ollama:qwen3.5:2b",
        localModel: "qwen3.5:2b",
        sidecarUp: true,
        installed: PREVIEW_INSTALLED,
        catalog: PREVIEW_CATALOG,
        cloudEntries: PREVIEW_CLOUD,
      });
      return;
    }
    try {
      const [cfg, installed, catalog, cloud] = await Promise.all([
        llmConfigGet(),
        llmModelListInstalled().catch(() => null),
        llmCatalog().catch(() => [] as CatalogRow[]),
        llmCloudEntriesGet().catch(() => [] as CloudEntryView[]),
      ]);
      set({
        loaded: true,
        provider: cfg.provider,
        activeId: cfg.activeId,
        localModel: cfg.localModel,
        sidecarUp: cfg.sidecarUp && installed !== null,
        installed: installed ?? [],
        catalog,
        cloudEntries: cloud,
        lastError: installed === null ? humanizeLlmError("sidecar_down") : null,
      });
    } catch (err) {
      set({ loaded: true, lastError: humanizeLlmError(err) });
    }
  },

  refreshInstalled: async () => {
    if (!inTauri) return;
    try {
      const installed = await llmModelListInstalled();
      set({ installed, sidecarUp: true, lastError: null });
    } catch (err) {
      set({ installed: [], sidecarUp: false, lastError: humanizeLlmError(err) });
    }
  },

  setModelTab: (tab) => set({ modelTab: tab }),

  loadCloud: async () => {
    if (!inTauri) return;
    try {
      set({ cloudEntries: await llmCloudEntriesGet(), lastError: null });
    } catch (err) {
      set({ lastError: humanizeLlmError(err) });
    }
  },

  activateId: async (id) => {
    if (get().testingId) return false;
    set({ testingId: id, lastError: null });

    // 浏览器预览：无后端，模拟一次探测让交互可演示（同 onboarding 的 demo 分支）
    if (!inTauri) {
      await new Promise((r) => setTimeout(r, 600));
      const label = id.startsWith("cloud:")
        ? get().cloudEntries.find((e) => `cloud:${e.id}` === id)?.name
        : id.replace(/^ollama:/, "");
      set({ activeId: id, testingId: null });
      if (!id.startsWith("cloud:")) set({ localModel: id.replace(/^ollama:/, "") });
      set({ provider: id.startsWith("cloud:") ? "cloud" : "ollama" });
      toast(id.startsWith("cloud:") ? `已切到云端「${label}」（预览）` : `已切到 ${label}（预览）`);
      return true;
    }

    try {
      const view = await llmConfigSetActive(id);
      set({
        provider: view.provider,
        activeId: view.activeId,
        localModel: view.localModel,
        testingId: null,
      });
      if (view.provider === "cloud") {
        const name = get().cloudEntries.find((e) => `cloud:${e.id}` === view.activeId)?.name;
        toast(`已切到云端「${name ?? ""}」`);
      } else {
        toast(`已切到 ${view.localModel}`);
      }
      return true;
    } catch (err) {
      // 失败还原：不留 testing 态、不改 activeId
      set({ testingId: null, lastError: humanizeLlmError(err) });
      return false;
    }
  },

  activate: async (tag) => get().activateId(`ollama:${tag}`),

  activateCloud: async (entryId) => get().activateId(`cloud:${entryId}`),

  saveCloudEntry: async (input) => {
    // 浏览器预览：没有后端，本地登记一条让交互可演示
    if (!inTauri) {
      const id = input.id ?? `demo-${Date.now()}`;
      const prev = get().cloudEntries.find((e) => e.id === id);
      const view: CloudEntryView = {
        id,
        name: input.name.trim(),
        kind: input.kind,
        baseUrl: input.baseUrl.trim(),
        model: input.model.trim(),
        maskedKey: input.key.trim() ? "••••" + input.key.trim().slice(-4) : (prev?.maskedKey ?? ""),
      };
      set((st) => ({
        lastError: null,
        cloudEntries: prev
          ? st.cloudEntries.map((e) => (e.id === id ? view : e))
          : [...st.cloudEntries, view],
      }));
      toast("已保存（预览）");
      return true;
    }
    try {
      const view = await llmCloudEntrySave(input);
      set((st) => {
        const exists = st.cloudEntries.some((e) => e.id === view.id);
        return {
          lastError: null,
          cloudEntries: exists
            ? st.cloudEntries.map((e) => (e.id === view.id ? view : e))
            : [...st.cloudEntries, view],
        };
      });
      toast(`已保存「${view.name}」`);
      return true;
    } catch (err) {
      set({ lastError: humanizeLlmError(err) });
      return false;
    }
  },

  removeCloudEntry: async (entryId) => {
    const wasActive = get().activeId === `cloud:${entryId}`;
    if (!inTauri) {
      set((st) => ({ cloudEntries: st.cloudEntries.filter((e) => e.id !== entryId) }));
      return;
    }
    try {
      await llmCloudEntryRemove(entryId);
      set((st) => ({ cloudEntries: st.cloudEntries.filter((e) => e.id !== entryId) }));
      if (wasActive) {
        // 后端已联动回落本地，这里同步前端状态
        set({ provider: "ollama", activeId: `ollama:${get().localModel}` });
        toast("删掉了激活的条目，已回落到本地模型");
      }
    } catch (err) {
      toast(humanizeLlmError(err));
    }
  },

  testCloudEntry: async (input) => {
    if (!inTauri) {
      await new Promise((r) => setTimeout(r, 500));
      return { latencyMs: 420 };
    }
    try {
      const report = await llmCloudTest(input);
      set({ lastError: null });
      return report;
    } catch (err) {
      set({ lastError: humanizeLlmError(err) });
      return null;
    }
  },

  download: async (tag) => {
    const inFlight = Object.values(get().pulls).some((p) => !TERMINAL.includes(p.status));
    if (inFlight) {
      toast("已经有一个下载在跑了");
      return;
    }
    set((st) => ({
      lastError: null,
      pulls: {
        ...st.pulls,
        [tag]: { tag, status: "pulling", percent: 0, completed: 0, total: 0 },
      },
    }));
    try {
      await llmModelDownload(tag);
    } catch (err) {
      get().clearPull(tag);
      set({ lastError: humanizeLlmError(err) });
    }
  },

  cancel: async () => {
    try {
      await llmModelCancel();
    } catch {
      /* 尽力而为：取消失败就让它在后台跑完 */
    }
  },

  remove: async (tag) => {
    try {
      await llmModelRemove(tag);
      toast(`删掉了 ${tag}`);
      await get().refreshInstalled();
    } catch (err) {
      toast(humanizeLlmError(err));
    }
  },

  applyPull: (p) =>
    set((st) => ({ pulls: { ...st.pulls, [p.tag]: p } })),

  clearPull: (tag) =>
    set((st) => {
      const rest = { ...st.pulls };
      delete rest[tag];
      return { pulls: rest };
    }),
}));

// 内部：本地/云端共用一条切换链路（探测 → 落库 → 应用）。
interface Switchable {
  activateId: (id: string) => Promise<boolean>;
}
