"use client";

// 云端 pane（spec-m2 §3）：已配置清单 + 添加按钮；点卡片选中（首次弹 opt-in），
// 编辑 / 删除复用同一套弹窗。
import { useState } from "react";
import { useLlmStore } from "@/lib/stores/llmStore";
import { useUiStore } from "@/lib/stores/uiStore";
import type { CloudEntryView } from "@/lib/types";
import { CloudEntryCard } from "./CloudEntryCard";
import { CloudFormModal } from "./CloudFormModal";
import { CloudOptinModal } from "./CloudOptinModal";

export function CloudSection() {
  const cloudEntries = useLlmStore((s) => s.cloudEntries);
  const activeId = useLlmStore((s) => s.activeId);
  const testingId = useLlmStore((s) => s.testingId);
  const lastError = useLlmStore((s) => s.lastError);
  const activateCloud = useLlmStore((s) => s.activateCloud);
  const saveCloudEntry = useLlmStore((s) => s.saveCloudEntry);
  const removeCloudEntry = useLlmStore((s) => s.removeCloudEntry);
  const testCloudEntry = useLlmStore((s) => s.testCloudEntry);
  const acked = useUiStore((s) => s.cloudOptinAcked);
  const ack = useUiStore((s) => s.ackCloudOptin);

  /** null = 关闭；`{}` = 添加；含 entry = 编辑 */
  const [form, setForm] = useState<CloudEntryView | Record<string, never> | null>(null);
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [confirmId, setConfirmId] = useState<string | null>(null);

  function pick(id: string) {
    if (!acked) {
      setPendingId(id);
      return;
    }
    void activateCloud(id);
  }

  const confirmTarget = cloudEntries.find((e) => e.id === confirmId) ?? null;

  return (
    <>
      <p className="hint-line" style={{ margin: "8px 0" }}>
        已配置的云端模型（点卡片选中生效 · 右上角可编辑 / 删除）
      </p>

      {lastError && <p className="model-error">{lastError}</p>}

      {cloudEntries.length === 0 ? (
        <p className="hint-line cloud-empty">还没有配置，点下面「＋ 添加模型」</p>
      ) : (
        <div className="clist">
          {cloudEntries.map((entry) => (
            <CloudEntryCard
              key={entry.id}
              entry={entry}
              active={activeId === `cloud:${entry.id}`}
              testing={testingId === `cloud:${entry.id}`}
              onPick={pick}
              onEdit={(e) => setForm(e)}
              onRemove={setConfirmId}
            />
          ))}
        </div>
      )}

      <button type="button" className="btn cloud-add" onClick={() => setForm({})}>
        ＋ 添加模型
      </button>
      <p className="hint-line" style={{ marginTop: 8 }}>
        支持 OpenAI 兼容端点（DeepSeek / Moonshot / GLM / Qwen 兼容模式 / OpenRouter）、
        OpenAI 官方与 Claude 官方。首次选中任一云端条目会弹出授权警示。
      </p>

      {pendingId && (
        <CloudOptinModal
          onCancel={() => setPendingId(null)}
          onConfirm={() => {
            const id = pendingId;
            ack();
            setPendingId(null);
            void activateCloud(id);
          }}
        />
      )}

      {form && (
        <CloudFormModal
          entry={"id" in form ? (form as CloudEntryView) : undefined}
          onClose={() => setForm(null)}
          onSave={saveCloudEntry}
          onTest={testCloudEntry}
        />
      )}

      {confirmTarget && (
        <div className="mconfirm" role="dialog" aria-modal="true">
          <div className="mconfirm-box">
            <p className="mconfirm-title">删除「{confirmTarget.name}」？</p>
            <p className="mconfirm-sub">
              {activeId === `cloud:${confirmTarget.id}`
                ? "这是当前生效的条目，删除后会回落到本地默认模型。"
                : "只删配置，不影响本机模型。"}
            </p>
            <div className="mconfirm-actions">
              <button type="button" className="btn-ghost" onClick={() => setConfirmId(null)}>
                算了
              </button>
              <button
                type="button"
                className="btn-danger"
                onClick={() => {
                  const id = confirmTarget.id;
                  setConfirmId(null);
                  void removeCloudEntry(id);
                }}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
