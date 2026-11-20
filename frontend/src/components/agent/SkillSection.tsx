"use client";

// Skill 管理卡（spec-s6 §2）：目录扫描 + 启停 + 拒载标红 + 预算提示。
import { useCallback, useEffect, useState } from "react";
import { humanizeAgentError, skillList, skillRemove, skillToggle } from "@/lib/commands";
import type { SkillView } from "@/lib/types";

export function SkillSection() {
  const [rows, setRows] = useState<SkillView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setRows(await skillList());
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

  const enabledCount = rows.filter((r) => r.enabled).length;

  return (
    <div className="card agent-card" data-testid="skill-section">
      <p className="card-title">技能包（Skill）</p>
      {error && <p className="model-error">{error}</p>}
      {rows.length === 0 ? (
        <p className="hint-line">
          把技能包放进应用数据目录的 skills/&lt;id&gt;/（SKILL.md ≤8KB + references/*.md ≤4KB×5），
          点「重新扫描」即可看到。
        </p>
      ) : (
        <div className="agent-list">
          {rows.map((s) => (
            <div key={s.id} className="agent-row" data-valid={s.valid}>
              <div className="agent-row-main">
                <span className="agent-name">{s.name}</span>
                <span className="hint-line">{s.valid ? s.description : "拒载"}</span>
              </div>
              <div className="agent-row-actions">
                {s.valid ? (
                  <>
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
        全文合计超预算会拖慢 2B 模型，按需启停。
      </p>
    </div>
  );
}
