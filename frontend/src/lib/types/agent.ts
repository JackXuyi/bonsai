// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

// ── agent（mcp_admin.rs / skill.rs / agent_tools.rs，spec-s2/s5/s4）──────
export interface ToolDescriptor {
  name: string;
  summary: string;
  detail: string;
  kind: "mcp" | "skill" | "builtin";
  remote: boolean;
  /** 权限档：read | ask | forbidden（缺省按 read）。 */
  access?: "read" | "ask" | "forbidden";
}

export interface McpServerRow {
  id: string;
  name: string;
  transport: "stdio" | "http";
  command?: string | null;
  args: string[];
  url?: string | null;
  headers: [string, string][];
  env: [string, string][];
  enabled: boolean;
  toolCount: number;
  toolCacheAt?: string | null;
}

export interface SkillView {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  valid: boolean;
  errorCode?: string | null;
  /** 自动提取草稿（plan-skill-auto-extract）。 */
  autoExtracted?: boolean | null;
  autoSource?: string | null;
  autoConfidence?: number | null;
}

export interface SkillAutoCandidate {
  taskId: string;
  goal: string;
  status: string;
  okTools: number;
  explicit: boolean;
  chainKey: string;
}

/** 技能预览：元信息 + 全文 + 已加载引用（skill_read）。 */
export interface SkillRefView {
  name: string;
  content: string;
}

export interface SkillDetail {
  id: string;
  name: string;
  description: string;
  /** frontmatter `version:`；缺省 null/undefined，预览显示「—」 */
  version?: string | null;
  body: string;
  refs: SkillRefView[];
  enabled: boolean;
  sizeBytes: number;
}

/** 手写创建技能包的入参（skill_create）。 */
export interface SkillCreateInput {
  id: string;
  name: string;
  description: string;
  body: string;
}

export interface ToolCallRecord {
  id: number;
  ts: string;
  tool: string;
  action: string;
  outcome: string;
  resultStatus?: string | null;
  paramsSummary?: string | null;
  promptTrace?: string | null;
  remote: boolean;
  durationMs?: number | null;
}

export interface McpSaveInput {
  id?: string;
  name: string;
  transport: "stdio" | "http";
  command?: string | null;
  args?: string[];
  url?: string | null;
  headers?: [string, string][];
  env?: [string, string][];
}

// ── 本机 MCP 自动发现（mcp_discover.rs，spec-s2 §6）──────────────
export interface DiscoveredServer {
  name: string;
  transport: "stdio" | "http";
  command?: string | null;
  args: string[];
  url?: string | null;
  /** 已脱敏（后端 mask_secrets），导入走后端重扫，前端不带明文。 */
  headers: [string, string][];
  env: [string, string][];
  source: string;
  signature: string;
  imported: boolean;
}

export interface McpImportReport {
  inserted: number;
  skipped: number;
}

// ── 计划进度（plan-planning.md §4.2 / §5.6，T6）────────────────
/** 落库契约（Rust `PlanJson`，嵌套字段保持 snake_case）。 */
export interface PlanStepRow {
  seq: number;
  intent: string;
  tool_hint?: string;
  status?: string;
  attempts?: number;
  tool_call_id?: number | null;
}

export interface PlanJsonRow {
  version?: number;
  goal: string;
  steps: PlanStepRow[];
  replans?: number;
}

/** `plan_list` 行（Rust `TaskRow`，camelCase）。 */
export interface PlanTaskRow {
  id: string;
  createdAt: string;
  goal: string;
  status: string;
  toolCalls: number;
  finalReply?: string | null;
  plan?: PlanJsonRow | null;
  planMode: string;
  cancelReason?: string | null;
  confirmMode: string;
  /** Phase 3 A1 */
  resumeCursor?: number | null;
  interruptKind?: string | null;
  resumable?: boolean;
}

/** `task_resume` / `task_reentry_choice` 返回。 */
export interface ResumePrepareResult {
  taskId: string;
  outcome: string;
  resumeCursor?: number | null;
  message: string;
  lastSafeSeq: number;
}

/** P3-5 `job_list` 行（Rust `JobRow`，camelCase）。 */
export interface JobRow {
  id: string;
  name: string;
  scheduleKind: string;
  scheduleExpr: string;
  enabled: boolean;
  lastRunAt?: string | null;
  lastStatus?: string | null;
  consecutiveFails: number;
  payloadJson: string;
}

/** 步骤态：库/事件里是 `done|skipped|failed` 与 `ok|denied|error` 两套，
 *  统一收敛到这一组（store 里映射，组件只认这一份）。 */
export type PlanStepStatus =
  | "pending"
  | "running"
  | "done"
  | "failed"
  | "skipped";

/** 面板视图模型：id/goal 来自 tasks，steps 由 plan_json + task_step 合并。 */
export interface PlanStepView {
  seq: number;
  intent: string;
  toolHint: string;
  status: PlanStepStatus;
  attempts: number;
  toolCallId?: number | null;
}

export interface PlanProgress {
  id: string;
  goal: string;
  /** tasks.status：running / done / failed / canceled / interrupted */
  status: string;
  cancelReason?: string | null;
  steps: PlanStepView[];
  /** Phase 3 A1：面板「恢复」 */
  resumable?: boolean;
  resumeCursor?: number | null;
  interruptKind?: string | null;
}
