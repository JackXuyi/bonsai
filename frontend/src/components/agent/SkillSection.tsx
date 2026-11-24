"use client";

// Skill 管理卡（spec-s6 §2）+ 自动提取草稿（plan-skill-auto-extract M2）。
import { useCallback, useEffect, useState } from "react";
import {
  humanizeAgentError,
  skillAutoCandidates,
  skillAutoDismiss,
  skillAutoGetEnabled,
  skillAutoSetEnabled,
  skillExtractNow,
  skillList,
  skillRemove,
  skillToggle,
} from "@/lib/commands";
import type { SkillAutoCandidate, SkillView } from "@/lib/types";
import { SkillAddDialog } from "./SkillAddDialog";
import { SkillPreviewDialog } from "./SkillPreviewDialog";

export function SkillSection() {
  const [rows, setRows] = useState<SkillView[]>([]);
  const [cands, setCands] = useState<SkillAutoCandidate[]>([]);
  const [autoOn, setAutoOn] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [extracting, setExtracting] = useState<string | null>(null);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [list, enabled, pending] = await Promise.all([
        skillList(),
        skillAutoGetEnabled(),
        skillAutoCandidates(),
      ]);
      setRows(list);
      setAutoOn(enabled);
      setCands(pending);
      setError(null);
    } catch (e) {
      setError(humanizeAgentError(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function toggle(id: string, enable: boolean) {
    setBusyId(id);
    try {
      await skillToggle(id, enable);
      setError(null);
      await refresh();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setBusyId(null);
    }
  }

  async function setAuto(next: boolean) {
    try {
      await skillAutoSetEnabled(next);
      setAutoOn(next);
      setError(null);
    } catch (e) {
      setError(humanizeAgentError(e));
    }
  }

  async function extract(taskId: string) {
    setExtracting(taskId);
    try {
      await skillExtractNow(taskId);
      setError(null);
      await refresh();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setExtracting(null);
    }
  }

  const enabledCount = rows.filter((r) => r.enabled).length;

  return (
    <div className="card agent-card" data-testid="skill-section">
      <div className="agent-approval">
        <p className="card-title">技能包（Skill）</p>
        <button
          type="button"
          className="btn-ghost"
          data-testid="skill-add-open"
          onClick={() => setAddOpen(true)}
        >
          ＋ 添加技能
        </button>
      </div>
      {error && <p className="model-error">{error}</p>}

      <div className="agent-row" data-testid="skill-auto-toggle">
        <div className="agent-row-main">
          <span className="agent-name">自动提取</span>
          <span className="hint-line">
            夜间从成功任务生成草稿（默认禁用）；启用后需云端对话才会注入技能
          </span>
        </div>
        <div className="agent-row-actions">
          <button
            type="button"
            className={autoOn ? "btn-danger" : "btn"}
            data-testid="skill-auto-enabled"
            onClick={() => void setAuto(!autoOn)}
          >
            {autoOn ? "关闭自动" : "开启自动"}
          </button>
        </div>
      </div>

      {cands.length > 0 && (
        <div className="agent-list" data-testid="skill-auto-candidates">
          <p className="hint-line">待提取候选</p>
          {cands.map((c) => (
            <div key={c.taskId} className="agent-row">
              <div className="agent-row-main">
                <span className="agent-name">{c.goal || c.taskId}</span>
                <span className="hint-line">
                  {c.explicit ? "显式固化 · " : ""}
                  {c.okTools} 次工具 · {c.chainKey || "—"}
                </span>
              </div>
              <div className="agent-row-actions">
                <button
                  type="button"
                  className="btn"
                  disabled={extracting === c.taskId}
                  data-testid={`skill-extract-${c.taskId}`}
                  onClick={() => void extract(c.taskId)}
                >
                  {extracting === c.taskId ? "生成中…" : "立即提取"}
                </button>
                <button
                  type="button"
                  className="btn-ghost"
                  onClick={async () => {
                    try {
                      await skillAutoDismiss(c.taskId);
                      await refresh();
                    } catch (e) {
                      setError(humanizeAgentError(e));
                    }
                  }}
                >
                  丢弃
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {rows.length === 0 ? (
        <p className="hint-line">
          把技能包放进应用数据目录的 skills/&lt;id&gt;/（SKILL.md ≤16KB + references/*.md ≤4KB×5），
          或点「＋ 添加技能」目录导入 / 手写创建 / 从 GitHub 等远端加载。列表实时扫描，无需手动刷新。
        </p>
      ) : (
        <div className="agent-list">
          {rows.map((s) => (
            <div key={s.id} className="agent-row" data-valid={s.valid}>
              <div className="agent-row-main">
                <span className="agent-name">{s.name}</span>
                <span className="hint-line">
                  {s.valid
                    ? s.autoExtracted && !s.enabled
                      ? `自动提取 · 未启用${s.autoSource ? ` · ${s.autoSource}` : ""}`
                      : s.description
                    : "拒载"}
                </span>
              </div>
              <div className="agent-row-actions">
                {s.valid ? (
                  <>
                    <button
                      type="button"
                      className="btn-ghost"
                      disabled={busyId === s.id}
                      data-testid={`skill-preview-${s.id}`}
                      onClick={() => setPreviewId(s.id)}
                    >
                      预览
                    </button>
                    <button
                      type="button"
                      className={s.enabled ? "btn-danger" : "btn"}
                      disabled={busyId === s.id}
                      onClick={() => void toggle(s.id, !s.enabled)}
                    >
                      {s.enabled ? "停用" : "启用"}
                    </button>
                    <button
                      type="button"
                      className="btn-ghost"
                      disabled={busyId === s.id}
                      onClick={async () => {
                        try {
                          await skillRemove(s.id);
                          await refresh();
                        } catch (e) {
                          setError(humanizeAgentError(e));
                        }
                      }}
                    >
                      删
                    </button>
                  </>
                ) : (
                  <span className="agent-invalid">
                    {s.errorCode === "skill_injection"
                      ? "试图改写人格边界，已拦截"
                      : s.errorCode === "skill_too_large"
                        ? "超出大小上限"
                        : "格式错误"}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
      <p className="hint-line" style={{ marginTop: 8 }}>
        已启用 {enabledCount} 个 · 启用即注入提示段（标注 untrusted，与人格宪法冲突时以宪法为准）；
        全文合计超预算会拖慢 2B 模型，按需启停。本地模式不会注入技能，需开启云端。
      </p>
      <SkillPreviewDialog
        open={previewId !== null}
        id={previewId}
        onClose={() => setPreviewId(null)}
      />
      <SkillAddDialog open={addOpen} onClose={() => setAddOpen(false)} onChanged={() => void refresh()} />
    </div>
  );
}
