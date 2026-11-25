// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

// ── background loops（plan-loop-supervisor / failure::loops_status）──
export interface LoopStatus {
  name: string;
  state: "running" | "stopped" | string;
  last_beat?: string | null;
  stopped_at?: string | null;
  detail?: string | null;
  restarts: number;
}

/** 设置页诊断行（plan-retry §7.4 / failure::failures_recent） */
export interface FailureRecent {
  source: "turn" | "consolidate" | string;
  kind: string;
  hint: string;
  at: string;
}

/** plan-store-lock T4：写锁采样（`store_lock_stats`，snake_case）。 */
export interface StoreLockStats {
  write_acquires: number;
  write_wait_ms_total: number;
  write_wait_slow: number;
  write_hold_ms_total: number;
  write_hold_samples: number;
  write_wait_ms_avg: number;
  write_hold_ms_avg: number;
}

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

// ── ability（ability.rs，P4-4）────────────────────────────────
export interface AbilityAnnouncement {
  id: number;
  stage: number;
  note: string;
}

export interface AbilityStatus {
  stage: number;
  stage_name?: string;
  day_index?: number;
  stage_since?: string;
  pending?: AbilityAnnouncement[];
}

// ── perception（perception.rs）────────────────────────────────
/** P2-4 教名词（src-tauri/src/perception.rs TaughtNoun）。 */
export interface TaughtNoun {
  needle: string;
  label: string;
}

export interface PerceptionConfig {
  enabled: boolean;
  clipboard: boolean;
  active_app: boolean;
  downloads: boolean;
  /** P2-4：窗口标题白名单采集，默认关。 */
  window_title?: boolean;
  title_allowlist?: string[];
  taught_nouns?: TaughtNoun[];
}

export interface PerceptionStatus {
  enabled: boolean;
  idle_secs?: number;
  front_app?: string;
  front_title?: string | null;
  window_title?: boolean;
  title_allowlist?: string[];
  taught_nouns?: TaughtNoun[];
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

/** P3-1 邮件状态（src-tauri/src/mail.rs MailStatus）。 */
export interface MailStatus {
  enabled: boolean;
  supported: boolean;
  today_reads: number;
  today_drafts: number;
  today_marks: number;
  note: string;
}

/** P4-3 / G-18 日时间线导出（src-tauri/src/observability.rs）。 */
export interface DayTimelineExport {
  day: string;
  markdown: string;
  turns: number;
  tools: number;
  security: number;
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
