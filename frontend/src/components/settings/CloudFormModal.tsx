"use client";

// 添加 / 编辑云端模型（spec-m2 §3）：名称 / 类型 / 接口地址 / API Key / 模型名
// + 内联「测试连接」。官方 OpenAI 与 Claude 锁定端点（spec-m2.1 §1）；
// 编辑态 key 留空 = 保持原 key。
import { useState } from "react";
import type {
  CloudEntryInput,
  CloudEntryKind,
  CloudEntryView,
  CloudTestReport,
} from "@/lib/types";

const OPENAI_BASE = "https://api.openai.com/v1";
// 与 Rust 侧 ANTHROPIC_BASE_URL 一致：不带 /v1，客户端自己拼 /v1/messages。
const ANTHROPIC_BASE = "https://api.anthropic.com";

const KIND_OPTIONS: { value: CloudEntryKind; label: string }[] = [
  { value: "custom", label: "自定义 OpenAI 兼容" },
  { value: "openai", label: "OpenAI 官方" },
  { value: "claude", label: "Claude 官方" },
];

/** 官方类型的固定端点；`custom` 返回 null（用户自己填）。 */
function officialBase(kind: CloudEntryKind): string | null {
  if (kind === "openai") return OPENAI_BASE;
  if (kind === "claude") return ANTHROPIC_BASE;
  return null;
}

/** 模型名的示例值跟着类型走，省一次查文档。 */
const MODEL_PLACEHOLDER: Record<CloudEntryKind, string> = {
  custom: "deepseek-chat",
  openai: "gpt-4o-mini",
  claude: "claude-3-5-sonnet-latest",
};

interface CloudFormModalProps {
  /** 编辑态：预填脱敏信息（key 不回填明文，留空即保持） */
  entry?: CloudEntryView;
  onClose: () => void;
  onSave: (input: CloudEntryInput) => Promise<boolean>;
  onTest: (input: CloudEntryInput) => Promise<CloudTestReport | null>;
}

type TestState =
  | { kind: "idle" }
  | { kind: "testing" }
  | { kind: "ok"; latencyMs: number }
  | { kind: "fail" };

export function CloudFormModal({ entry, onClose, onSave, onTest }: CloudFormModalProps) {
  const [name, setName] = useState(entry?.name ?? "");
  const [kind, setKind] = useState<CloudEntryKind>(entry?.kind ?? "custom");
  const [baseUrl, setBaseUrl] = useState(entry?.baseUrl ?? "");
  const [model, setModel] = useState(entry?.model ?? "");
  const [key, setKey] = useState("");
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const official = officialBase(kind);

  function build(): CloudEntryInput | null {
    const trimmedName = name.trim();
    const trimmedModel = model.trim();
    if (!trimmedName) {
      setError("名字不能空");
      return null;
    }
    if (!trimmedModel) {
      setError("模型名不能空");
      return null;
    }
    if (!official && !/^https?:\/\/[^\s/]+/i.test(baseUrl.trim())) {
      setError("接口地址要是 http(s) 开头的完整地址");
      return null;
    }
    // 编辑态留空 = 保持原 key（后端同样按此处理）
    if (!entry && !key.trim()) {
      setError("API Key 不能空");
      return null;
    }
    return {
      id: entry?.id,
      name: trimmedName,
      kind,
      baseUrl: official ?? baseUrl.trim(),
      model: trimmedModel,
      key: key.trim(),
    };
  }

  async function runTest() {
    const input = build();
    if (!input) return;
    setError(null);
    setTest({ kind: "testing" });
    const report = await onTest(input);
    setTest(report ? { kind: "ok", latencyMs: report.latencyMs } : { kind: "fail" });
  }

  async function save() {
    const input = build();
    if (!input || saving) return;
    setSaving(true);
    setError(null);
    const ok = await onSave(input);
    setSaving(false);
    if (ok) onClose();
  }

  return (
    <div className="mconfirm" role="dialog" aria-modal="true" aria-label="云端模型配置">
      <div className="mconfirm-box form">
        <p className="mconfirm-title">{entry ? "编辑云端模型" : "添加云端模型"}</p>

        <label className="field">
          <span>名称</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="例：阿盆的 DeepSeek"
            autoFocus
          />
        </label>

        <label className="field">
          <span>类型</span>
          <select value={kind} onChange={(e) => setKind(e.target.value as CloudEntryKind)}>
            {KIND_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>

        {official ? (
          <p className="hint-line">官方端点固定为 {official}</p>
        ) : (
          <label className="field">
            <span>接口地址（/v1）</span>
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://api.deepseek.com/v1"
            />
          </label>
        )}

        <label className="field">
          <span>API Key</span>
          <input
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={entry ? "留空保持原 key" : "sk-…"}
          />
        </label>

        <label className="field">
          <span>模型名</span>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={MODEL_PLACEHOLDER[kind]}
          />
        </label>

        <div className={`test-line ${test.kind}`} role="status">
          {test.kind === "testing" && "测试中…"}
          {test.kind === "ok" && `✓ 测试通过 · 1 token · ${(test.latencyMs / 1000).toFixed(1)}s`}
          {test.kind === "fail" && "✕ 没通过：检查地址 / Key / 模型名"}
          {error && <span className="test-error">{error}</span>}
        </div>

        <div className="mconfirm-actions">
          <button type="button" className="btn-ghost" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn" onClick={runTest} disabled={test.kind === "testing"}>
            测试连接
          </button>
          <button type="button" className="btn" onClick={save} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
