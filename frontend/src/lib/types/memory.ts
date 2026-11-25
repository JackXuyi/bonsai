// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

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

