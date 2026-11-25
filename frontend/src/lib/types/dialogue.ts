// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

// ── dialogue / session / prediction / consolidate ──
// ── dialogue（dialogue.rs DialogueOutcome）──────────────────────
export interface DialogueFilter {
  unconditional_affirmation?: boolean;
  hedged_disagreement?: boolean;
}

/** 轮层失败（plan-retry T7）：kind 决定文案，不再靠字符串 includes。 */
export interface TurnFailure {
  kind: string;
  message: string;
  retryable: boolean;
}

export interface DialogueOutcome {
  reply: string;
  crisis?: boolean;
  /** None | "agenda_today"… | "mcp:<tool>" | "skill:<name>" | "builtin:memory_search"（spec-s3 + P3 内置工具） */
  tool?: string | null;
  model_used?: boolean;
  filter?: DialogueFilter;
  /** 本回合链路追踪 id（spec-s8），气泡「查看详情」入口 */
  turn_id?: number | null;
  /** 本轮失败：有值时气泡下方出失败行；401/429/超时各说各话 */
  failure?: TurnFailure | null;
}

/** 主动搭话手动触发的返回（spec-s8：文本 + trace turn id）。 */
export interface ProactiveReply {
  text: string;
  turnId: number;
}

/** 链路追踪单步（trace_steps 行，spec-s8）。 */
export interface TraceStep {
  seq: number;
  stepKind:
    | "user_input"
    | "crisis_check"
    | "tool_intent"
    | "summary_intent"
    | "context_budget"
    | "tools_truncated"
    | "llm_round"
    | "tool_call"
    | "json_retry"
    | "prose_retry"
    | "filter"
    | "plan_submit"
    | "step_state"
    | "retry";
  title: string;
  prompt?: string | null;
  output?: string | null;
  argsJson?: string | null;
  summary?: string | null;
  outcome?: string | null;
  durationMs?: number | null;
  /** 当前输入（user message——不在 system prompt 里，v11 起每步自带） */
  userText?: string | null;
  ts: string;
}

/** 链路追踪回合主档 + 有序步骤（turn_trace 行，spec-s8）。 */
export interface TurnTrace {
  id: number;
  sessionId: string;
  userEventId?: number | null;
  assistantEventId?: number | null;
  kind: "model" | "tool" | "crisis" | "proactive";
  modelUsed: boolean;
  toolLabel?: string | null;
  filterReport?: string | null;
  durationMs?: number | null;
  /** 用户原文 / 最终回复（摘要卡显示；proactive 无 userText） */
  userText?: string | null;
  replyText?: string | null;
  ts: string;
  steps: TraceStep[];
}

/** 对话页启动回放的单条消息（events 表 kind=dialogue 平铺行，旧→新）。 */
export interface DialogueBubble {
  id: number;
  ts: string;
  role: "user" | "assistant";
  content: string;
}

/** 会话索引行（session.rs / sessions 表，plan-context.md §6.7）。
 *  `title` 空串 = 未命名（显示层兜「新对话」，后端不造中文占位串）。 */
export interface SessionRow {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  archived: boolean;
  messageCount: number;
  digest?: string | null;
  digestUptoEventId?: number | null;
}

// ── prediction（predict.rs）───────────────────────────────────
export interface PredictionAskedPayload {
  id: number;
  guess: string;
}

// ── memory（memory.rs consolidate 等）──────────────────────────
export interface ConsolidateResult {
  trigger: string;
  events_in_window: number;
  extracted: number;
  merged: number;
  contradicted: number;
  promoted: number;
  deleted: number;
  consumed: number;
}

