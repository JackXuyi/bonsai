// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。

// ── llm（llm_admin.rs，spec §A1 起新模块用 camelCase）──────────
export type LlmProvider = "ollama" | "cloud";

export interface LlmConfigView {
  provider: LlmProvider;
  localModel: string;
  activeId: string;
  /** true = Auto：后台任务只用本地模型（plan-structured-routing） */
  backgroundLocalOnly?: boolean;
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
