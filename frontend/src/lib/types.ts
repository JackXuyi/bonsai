// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

// ── face ──────────────────────────────────────────────────────
export type FaceState =
  | "calm"
  | "focused"
  | "sleepy"
  | "greeted"
  | "displeased"
  | "thinking"
  | "proud";

export const FACE_STATES: FaceState[] = [
  "calm",
  "focused",
  "sleepy",
  "greeted",
  "displeased",
  "thinking",
  "proud",
];

// ── growth（morph.rs growth_status / growth:stage）──────────────
export interface GrowthAnnouncement {
  id: number;
  stage: number;
  note: string;
}

export interface GrowthStatus {
  stage: number;
  dormant: boolean;
  day_index?: number;
  micro_seed?: number;
  pending?: GrowthAnnouncement[];
}

// ── relationship（relationship.rs）────────────────────────────
export interface RelationshipAnnouncement {
  id: number;
  kind: string;
  stage: number;
  note: string;
}

export interface RelationshipStatus {
  stage: number;
  stage_name?: string;
  day_index?: number;
  cold?: boolean;
  paused?: boolean;
  pause_reason?: string;
  pending?: RelationshipAnnouncement[];
}

// ── perception（perception.rs）────────────────────────────────
export interface PerceptionConfig {
  enabled: boolean;
  clipboard: boolean;
  active_app: boolean;
  downloads: boolean;
}

export interface PerceptionStatus {
  enabled: boolean;
  idle_secs?: number;
  front_app?: string;
}

// ── calendar（calendar.rs）────────────────────────────────────
export interface CalendarNextEvent {
  start_at: string;
  title: string;
}

export interface CalendarStatus {
  enabled: boolean;
  remind?: boolean;
  access: string;
  can_read?: boolean;
  cache_count?: number;
  next?: CalendarNextEvent | null;
  today_reads?: number;
  today_reminds?: number;
}

export interface CalendarRemindPayload {
  title: string;
  minutes_left: number;
}

/** P3-2 文件工具状态（src-tauri/src/filetools.rs FsStatus）。 */
export interface FsStatus {
  enabled: boolean;
  roots: string[];
  cloud_read: boolean;
}

/** P3-3 Web 检索状态（src-tauri/src/webtools.rs WebStatus）。 */
export interface WebStatus {
  enabled: boolean;
}

/** 表情自定义皮肤 manifest（src-tauri/src/faceskins.rs FaceSkinManifest）。
 *  slots: 表情槽位（FACE_STATES）→ 槽位文件名；缺键 = 未上传。 */
export interface FaceSkinManifest {
  id: string;
  name: string;
  created_at: number;
  slots: Record<string, string>;
}

// ── onboarding（onboarding.rs）────────────────────────────────
export interface OnboardingQuestion {
  id: string;
  kind: "text" | "choice";
  prompt: string;
  hint?: string;
  options: string[];
}

export interface OnboardingStatus {
  done: boolean;
  age_group?: string;
  questions?: OnboardingQuestion[];
}

export interface OnboardingAnswer {
  question: string;
  answer: string;
  skipped: boolean;
}

export interface OnboardingSubmitResult {
  minor: boolean;
  boundaries_added: number;
  preferences_added: number;
}

export interface Boundary {
  id: number;
  content: string;
}

// ── 用户画像（memory/profile.rs ProfileRow，plan-user-profile.md §6.3）──
export type ProfileSource = "rule" | "model" | "manual";

export interface ProfileEntry {
  key: string;
  value: string;
  source: ProfileSource;
  confidence: number;
  updatedAt: string;
}

// ── dialogue（dialogue.rs DialogueOutcome）──────────────────────
export interface DialogueFilter {
  unconditional_affirmation?: boolean;
  hedged_disagreement?: boolean;
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

// ── llm（llm_admin.rs，spec §A1 起新模块用 camelCase）──────────
export type LlmProvider = "ollama" | "cloud";

export interface LlmConfigView {
  provider: LlmProvider;
  localModel: string;
  activeId: string;
  sidecarUp: boolean;
  maskedKey?: string;
}

export interface OllamaModelRow {
  name: string;
  sizeBytes: number;
  modified: string;
}

export interface CatalogRow {
  tag: string;
  sizeLabel: string;
  sizeBytes: number;
  note: string;
  thinkOk: boolean;
  ramHint?: string;
  recommended: boolean;
}

export type PullStatus =
  | "pulling"
  | "warn_disk"
  | "done"
  | "error"
  | "cancelled";

export interface PullProgress {
  tag: string;
  status: PullStatus;
  percent: number;
  completed: number;
  total: number;
  error?: string;
}

// ── 云端模型（llm_cloud.rs，spec-m2）──────────────────────────
export type CloudEntryKind = "custom" | "openai" | "claude";

/** 清单视图：key 已被后端脱敏，前端拿不到明文。 */
export interface CloudEntryView {
  id: string;
  name: string;
  kind: CloudEntryKind;
  baseUrl: string;
  model: string;
  maskedKey: string;
}

/** 表单提交（id 存在 = 编辑；key 留空 = 保持原 key）。 */
export interface CloudEntryInput {
  id?: string;
  name: string;
  kind: CloudEntryKind;
  baseUrl: string;
  model: string;
  key: string;
}

export interface CloudTestReport {
  latencyMs: number;
}

// ── memory overview（memory/overview.rs，spec-m3）──────────────
export interface KindCount {
  kind: string;
  count: number;
}

export interface MemoryCounts {
  events: number;
  eventsByKind: KindCount[];
  facts: number;
  preferences: number;
  boundaries: number;
  /** 命中率分母：所有问过的预测（含没回复的） */
  predictionsTotal: number;
  /** 分子：feedback = 'hit' */
  predictionsHit: number;
  toolCalls: number;
  calendarCache: number;
}

export interface MemoryEventRow {
  id: number;
  ts: string;
  kind: string;
  content: string;
  role: string | null;
  /** 派生事件（face_state / disagreement / relationship_*）：前端灰显 */
  derived: boolean;
}

export interface MemoryFactRow {
  id: number;
  ts: string;
  content: string;
  confidence: number;
}

export interface MemoryPrefRow {
  id: number;
  ts: string;
  category: string;
  content: string;
  confidence: number;
}

export interface MemoryPredictionRow {
  id: number;
  date: string;
  guess: string;
  feedback: "hit" | "miss" | "shut_up" | string | null;
  askedAt: string | null;
  skipReason: string | null;
}

export interface ConsolidateRunRow {
  startedAt: string;
  finishedAt: string | null;
  trigger: string;
  status: string;
  extracted: number;
  merged: number;
  promoted: number;
  consumed: number;
}

export interface MemoryOverview {
  counts: MemoryCounts;
  recentEvents: MemoryEventRow[];
  recentFacts: MemoryFactRow[];
  recentPreferences: MemoryPrefRow[];
  recentPredictions: MemoryPredictionRow[];
  lastConsolidate: ConsolidateRunRow | null;
}

// ── persona snapshot（persona/snapshot.rs，spec-m4 §4A）─────────
export interface PersonaCardView {
  name: string;
  tone: string;
  constitutionItems: string[];
  bannedForms: string[];
}

export type RelationshipMode = "reserved" | "familiar" | "direct" | "rusty";

export interface PersonaSnapshot {
  card: PersonaCardView;
  relationshipMode: RelationshipMode;
  relationshipStage: number;
  faceState: string;
  growthStage: number;
  dormant: boolean;
}

// ── debug 日志（debug.rs，spec-m4 §4B）──────────────────────────
export type LogFile = "app_log" | "sidecar_log";

// ── agent（mcp_admin.rs / skill.rs / agent_tools.rs，spec-s2/s5/s4）──────
export interface ToolDescriptor {
  name: string;
  summary: string;
  detail: string;
  kind: "mcp" | "skill" | "builtin";
  remote: boolean;
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
}

/** 技能预览：全文 + 已加载引用（skill_read）。 */
export interface SkillRefView {
  name: string;
  content: string;
}

export interface SkillDetail {
  id: string;
  name: string;
  description: string;
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
}

/** 步骤态：库/事件里是 `done|skipped|failed` 与 `ok|denied|error` 两套，
 *  统一收敛到这一组（store 里映射，组件只认这一份）。 */
export type PlanStepStatus = "pending" | "running" | "done" | "failed" | "skipped";

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
  /** tasks.status：running / done / failed / canceled */
  status: string;
  cancelReason?: string | null;
  steps: PlanStepView[];
}
