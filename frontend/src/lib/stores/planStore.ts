"use client";

// 计划进度（docs/zh/plan-planning.md §5.6，T6）：工具页只读面板的唯一状态源。
// 两条通道合成一份状态，避免「事件实时态」与「plan_list 回放态」互相打架：
//   回放 = plan_list（tasks 表是权威终态）
//   实时 = Channel 上的 task_start / task_step / task_done / task_canceled
// 合并规则：步骤语义（intent/toolHint）回放优先，状态只许「升阶」——
// pending < running < 终态，迟到的旧事件不会把 done 打回 running。
// 过程**不进消息流**（2026-11-22 裁决）：useChat 把事件转到这里后 return，
// chatStore.messages 零变化（A15 由 useChat 测试锁定）。
import { create } from "zustand";
import { planList } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import type {
  PlanProgress,
  PlanStepStatus,
  PlanStepView,
  PlanTaskRow,
} from "@/lib/types";

/** 面板只看最近几轮：与后端 plan_list 的默认窗口一致。 */
export const PLAN_HISTORY_CAP = 10;

/** 状态阶：终态同阶（done/failed/skipped 不再互相降级）。 */
const RANK: Record<PlanStepStatus, number> = {
  pending: 0,
  running: 1,
  done: 2,
  failed: 2,
  skipped: 2,
};

const TERMINAL_TASK = new Set(["done", "failed", "canceled"]);

/** 库/事件两套词都收敛：`ok|denied|error`（tool_calls 侧）与
 *  `done|skipped|failed`（plan_json 侧）。未知值按 pending 处理。 */
export function normalizeStepStatus(raw: string): PlanStepStatus {
  switch (raw) {
    case "ok":
      return "done";
    case "denied":
      return "skipped";
    case "error":
      return "failed";
    case "running":
      return "running";
    case "done":
    case "failed":
    case "skipped":
      return raw;
    default:
      return "pending";
  }
}

/** Channel 事件的最小形状（task_step 不带计划语义，只有 id/seq/tool/status）。 */
interface TaskEvent {
  event: string;
  id: string;
  goal?: string;
  seq?: number;
  tool?: string;
  status?: string;
  reason?: string;
  /** 只有计划分支的 task_start 带 true；单步任务（无计划）不进本面板 */
  planned?: boolean;
}

function asEvent(raw: unknown, fallbackId: string | null): TaskEvent | null {
  if (!raw || typeof raw !== "object") return null;
  const o = raw as Record<string, unknown>;
  if (typeof o.event !== "string") return null;
  // task_canceled 只有 reason（后端不带 id）：归到当前任务上。
  const id =
    typeof o.id === "string" && o.id ? o.id : o.event === "task_canceled" ? fallbackId : null;
  if (!id) return null;
  return {
    event: o.event,
    id,
    goal: typeof o.goal === "string" ? o.goal : undefined,
    seq: typeof o.seq === "number" ? o.seq : undefined,
    tool: typeof o.tool === "string" ? o.tool : undefined,
    status: typeof o.status === "string" ? o.status : undefined,
    reason: typeof o.reason === "string" ? o.reason : undefined,
    planned: typeof o.planned === "boolean" ? o.planned : undefined,
  };
}

/** 按 seq 合并：语义取先有的非空值，状态只升阶，attempts 取最大。 */
function mergeSteps(base: PlanStepView[], live: PlanStepView[]): PlanStepView[] {
  const map = new Map<number, PlanStepView>();
  for (const s of base) map.set(s.seq, s);
  for (const s of live) {
    const prev = map.get(s.seq);
    if (!prev) {
      map.set(s.seq, s);
      continue;
    }
    map.set(s.seq, {
      seq: s.seq,
      intent: s.intent || prev.intent,
      toolHint: s.toolHint || prev.toolHint,
      status: RANK[s.status] > RANK[prev.status] ? s.status : prev.status,
      attempts: Math.max(s.attempts, prev.attempts),
      toolCallId: s.toolCallId ?? prev.toolCallId,
    });
  }
  return [...map.values()].sort((a, b) => a.seq - b.seq);
}

/** 终态收口：取消时未跑完的步骤按后端语义标 skipped（不伪造 done）。 */
function settleSteps(steps: PlanStepView[], taskStatus: string): PlanStepView[] {
  if (taskStatus !== "canceled") return steps;
  return steps.map((s) =>
    s.status === "pending" || s.status === "running" ? { ...s, status: "skipped" } : s,
  );
}

/** plan_list 行 → 视图模型。plan_json 嵌套字段是 snake_case（落库契约）。 */
export function progressFromRow(row: PlanTaskRow): PlanProgress {
  const steps: PlanStepView[] = (row.plan?.steps ?? []).map((s) => ({
    seq: s.seq,
    intent: s.intent,
    toolHint: s.tool_hint ?? "",
    status: normalizeStepStatus(s.status ?? ""),
    attempts: s.attempts ?? 0,
    toolCallId: s.tool_call_id ?? null,
  }));
  return {
    id: row.id,
    goal: row.plan?.goal || row.goal,
    status: row.status,
    cancelReason: row.cancelReason ?? null,
    steps: steps.sort((a, b) => a.seq - b.seq),
    resumable: row.resumable ?? false,
    resumeCursor: row.resumeCursor ?? null,
    interruptKind: row.interruptKind ?? null,
  };
}

/** 面板只关心有计划的任务；单步任务（plan_mode=none）不进这张卡。 */
function isPlanned(row: PlanTaskRow): boolean {
  return row.planMode === "planned" || (row.plan?.steps.length ?? 0) > 0;
}

interface PlanState {
  /** 面板主角：执行中的那条，否则最近一条 */
  current: PlanProgress | null;
  /** 最近计划（新→旧），首屏回放用 */
  history: PlanProgress[];
  /** 实时通道：Channel 事件（useChat 分流后调用，不写消息流） */
  applyTaskEvent: (raw: unknown) => void;
  /** 回放通道：plan_list 结果（终态以 tasks 表为准，与实时态合并） */
  setHistory: (rows: PlanTaskRow[]) => void;
  /** 拉一次 plan_list；非 Tauri / 失败时静默降级为空面板 */
  refresh: () => Promise<void>;
}

export const usePlanStore = create<PlanState>((set, get) => ({
  current: null,
  history: [],

  applyTaskEvent: (raw) => {
    const fallbackId = get().current?.id ?? null;
    const e = asEvent(raw, fallbackId);
    if (!e) return;
    set((st) => {
      const existing =
        st.current?.id === e.id ? st.current : st.history.find((p) => p.id === e.id);
      const base: PlanProgress = existing ?? {
        id: e.id,
        goal: "",
        status: "running",
        cancelReason: null,
        steps: [],
      };
      let next: PlanProgress;
      switch (e.event) {
        case "task_start":
          // 单步任务（无计划的 task_start）不属于「计划进度」：不建条目，
          // 否则面板会被一堆 0 步的条目填满。
          if (!existing && e.planned !== true) return st;
          next = {
            ...base,
            goal: e.goal?.trim() ? e.goal.trim() : base.goal,
            status: "running",
          };
          break;
        case "task_step": {
          // 没有计划条目（含单步任务）就没有进度可画，不凭事件凭空造条目。
          if (!existing) return st;
          if (typeof e.seq !== "number" || e.seq < 0) return st;
          // task_step 只有 tool/status：seq 对齐到回放态已有步骤，缺则先占位
          // （intent 由随后的 plan_list 补齐，不在此编造）。
          const live: PlanStepView = {
            seq: e.seq,
            intent: "",
            toolHint: e.tool ?? "",
            status: normalizeStepStatus(e.status ?? ""),
            attempts: 1,
            toolCallId: null,
          };
          next = {
            ...base,
            status: TERMINAL_TASK.has(base.status) ? base.status : "running",
            steps: mergeSteps(base.steps, [live]),
          };
          break;
        }
        case "task_done": {
          const status = e.status || "done";
          next = { ...base, status, steps: settleSteps(base.steps, status) };
          break;
        }
        case "task_canceled":
          next = {
            ...base,
            status: "canceled",
            cancelReason: e.reason ?? "user",
            steps: settleSteps(base.steps, "canceled"),
          };
          break;
        default:
          return st; // 未知事件一律忽略，面板不猜
      }
      const history = [next, ...st.history.filter((p) => p.id !== next.id)].slice(
        0,
        PLAN_HISTORY_CAP,
      );
      return { current: next, history };
    });
  },

  setHistory: (rows) => {
    set((st) => {
      const live = new Map<string, PlanProgress>();
      if (st.current) live.set(st.current.id, st.current);
      for (const p of st.history) if (!live.has(p.id)) live.set(p.id, p);
      const merged = rows.filter(isPlanned).map((row) => {
        const base = progressFromRow(row);
        const prior = live.get(row.id);
        if (!prior) return base;
        return {
          ...base,
          goal: base.goal || prior.goal,
          cancelReason: base.cancelReason ?? prior.cancelReason,
          steps: mergeSteps(base.steps, prior.steps),
        };
      });
      // 本次会话收到过事件、回放里却没有（DB 尚未落盘 / 被窗口裁掉）的计划
      // 不被一次刷新抹掉——丢了就是进度倒退，比多一行更糟。
      const seen = new Set(merged.map((p) => p.id));
      for (const p of live.values()) {
        if (!seen.has(p.id)) merged.unshift(p);
      }
      merged.splice(PLAN_HISTORY_CAP);
      const current = merged.find((p) => p.status === "running") ?? merged[0] ?? null;
      return { current, history: merged };
    });
  },

  refresh: async () => {
    if (!inTauri) return;
    try {
      get().setHistory(await planList(PLAN_HISTORY_CAP));
    } catch {
      // 面板是增益功能：读不到就当没有，不弹错、不打扰。
    }
  },
}));
