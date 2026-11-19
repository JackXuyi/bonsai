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

// ── dialogue（dialogue.rs DialogueOutcome）──────────────────────
export interface DialogueFilter {
  unconditional_affirmation?: boolean;
  hedged_disagreement?: boolean;
}

export interface DialogueOutcome {
  reply: string;
  crisis?: boolean;
  tool?: boolean;
  model_used?: boolean;
  filter?: DialogueFilter;
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
