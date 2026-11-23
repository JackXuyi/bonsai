"use client";

// 会话索引（docs/zh/plan-context.md §6.8、§8）：对话页抽屉的唯一状态源。
//
// 三条边界：
// - 会话只是**对话窗口**：切会话切的是历史与摘要；记忆（facts / preferences /
//   用户画像）全局不变，所以这里不碰 memory 侧任何 store。
// - 消息流不在这里：切会话时由 ChatPane 负责 `chatStore.reset()` 再回放，
//   保证「旧气泡一个不留」（A12）；本 store 只管索引与当前指针。
// - `proactive` 会话不进列表（D6）：它永远不建行，后端也不会返回它。
import { create } from "zustand";
import {
  sessionArchive,
  sessionCreate,
  sessionDelete,
  sessionList,
  sessionRename,
} from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast } from "./uiStore";
import type { SessionRow } from "@/lib/types";

/** 老数据的锚点会话：不可删、不可归档（后端同样拒绝）。 */
export const DEFAULT_SESSION_ID = "default";
/** 未命名会话的显示名（后端存空串，不发明中文占位串）。 */
export const NEW_SESSION_TITLE = "新对话";
/** 改名上限与后端 `MANUAL_TITLE_MAX_CHARS` 一致，少一次往返截断。 */
export const TITLE_MAX_CHARS = 40;

/** 显示名：空标题 / 行不存在（刚清库、列表未回）都兜「新对话」。 */
export function sessionTitle(row?: Pick<SessionRow, "title"> | null): string {
  return row?.title.trim() || NEW_SESSION_TITLE;
}

/** 相对日：今天 / 昨天 / MM-DD（不做「x 天前」——列表只看新旧顺序）。 */
export function dayLabel(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const key = (x: Date) => `${x.getFullYear()}/${x.getMonth()}/${x.getDate()}`;
  if (key(d) === key(now)) return "今天";
  const y = new Date(now);
  y.setDate(y.getDate() - 1);
  if (key(d) === key(y)) return "昨天";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** 主列表 / 归档区分组（后端一次全给时用）。 */
export function splitSessions(list: SessionRow[]): {
  active: SessionRow[];
  archived: SessionRow[];
} {
  return {
    active: list.filter((s) => !s.archived),
    archived: list.filter((s) => s.archived),
  };
}

interface SessionState {
  /** 全部会话（含归档；分组交给 `splitSessions`） */
  list: SessionRow[];
  currentId: string;
  /** 拉过一次列表（无论成功与否）——首次进抽屉不闪空态 */
  loaded: boolean;
  /** CRUD 在飞：抽屉按钮统一置灰，防连点造重复会话 */
  pending: boolean;
  load: () => Promise<void>;
  switchTo: (id: string) => void;
  create: () => Promise<string | null>;
  rename: (id: string, title: string) => Promise<void>;
  archive: (id: string, archived: boolean) => Promise<void>;
  /** 删除（带合规级联，后端 forget_session）：返回删掉的事件条数 */
  remove: (id: string) => Promise<number>;
}

export const useSessionStore = create<SessionState>((set, get) => ({
  list: [],
  currentId: DEFAULT_SESSION_ID,
  loaded: false,
  pending: false,

  load: async () => {
    if (!inTauri) {
      set({ loaded: true });
      return;
    }
    try {
      // 一次全拉（含归档）：抽屉里归档区要显示条数，分两次拉反而多一次往返。
      const rows = await sessionList(true);
      set({ list: rows, loaded: true });
    } catch {
      // 列表是增益：读不到就当只有当前会话，不弹错、不挡对话。
      set({ loaded: true });
    }
  },

  switchTo: (id) => {
    if (get().currentId === id || !id) return;
    set({ currentId: id });
  },

  create: async () => {
    if (!inTauri) return null;
    set({ pending: true });
    try {
      const row = await sessionCreate();
      set((st) => ({
        list: [row, ...st.list.filter((r) => r.id !== row.id)],
        currentId: row.id,
      }));
      return row.id;
    } catch (e) {
      toast(`新建会话失败：${e}`);
      return null;
    } finally {
      set({ pending: false });
    }
  },

  rename: async (id, title) => {
    const next = title.trim().slice(0, TITLE_MAX_CHARS);
    // 空标题不提交（保留原名）：改名不是删除，用户清空多半是手滑。
    if (!next || !inTauri) return;
    set({ pending: true });
    try {
      await sessionRename(id, next);
      set((st) => ({
        list: st.list.map((r) => (r.id === id ? { ...r, title: next } : r)),
      }));
    } catch (e) {
      toast(`改名没成功：${e}`);
    } finally {
      set({ pending: false });
    }
  },

  archive: async (id, archived) => {
    if (!inTauri) return;
    set({ pending: true });
    try {
      await sessionArchive(id, archived);
      set((st) => ({
        list: st.list.map((r) => (r.id === id ? { ...r, archived } : r)),
      }));
    } catch (e) {
      toast(`归档没成功：${e}`);
    } finally {
      set({ pending: false });
    }
  },

  remove: async (id) => {
    if (!inTauri) return 0;
    set({ pending: true });
    try {
      const removed = await sessionDelete(id);
      set((st) => {
        const list = st.list.filter((r) => r.id !== id);
        // 删掉的正是当前会话：回到默认会话（它是老数据锚点，永远在）。
        const currentId = st.currentId === id ? DEFAULT_SESSION_ID : st.currentId;
        return { list, currentId };
      });
      return removed;
    } catch (e) {
      toast(`删除没成功：${e}`);
      throw e;
    } finally {
      set({ pending: false });
    }
  },
}));

/** 当前会话行：列表里没有（刚清库 / 列表还没回来）时返回 undefined，
 *  显示层用 `sessionTitle(row)` 兜「新对话」，不必自己分支。 */
export function useCurrentSession(): SessionRow | undefined {
  return useSessionStore((s) => s.list.find((r) => r.id === s.currentId));
}
