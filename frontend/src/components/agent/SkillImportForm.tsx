"use client";

// 目录导入表单：系统文件夹选择器（目录名即 id，含 SKILL.md）。
import { useState } from "react";
import { fsPickDirectory, humanizeAgentError, skillImport } from "@/lib/commands";

export function SkillImportForm({ onDone }: { onDone: () => void }) {
  const [source, setSource] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function pickDir() {
    const picked = await fsPickDirectory();
    if (picked) {
      setSource(picked);
      setError(null);
    }
  }

  async function submit() {
    if (!source.trim()) {
      setError("先用「选择目录」挑一个含 SKILL.md 的技能包文件夹");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await skillImport(source.trim());
      onDone();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <p className="skill-note">
        选一个含 <code>SKILL.md</code>（≤16KB，可选 <code>references/*.md</code>）的文件夹，
        目录名即技能 id（小写字母/数字/连字符）。导入先过运行时同一套校验，不通过就整体不落盘。
      </p>
      {error && <p className="model-error">{error}</p>}
      <label className="agent-field">
        技能包目录
        <div className="skill-input-row">
          <input
            data-testid="skill-import-source"
            value={source}
            onChange={(e) => setSource(e.target.value)}
            placeholder="/path/to/my-skill"
          />
          <button type="button" className="btn-ghost" disabled={busy} onClick={() => void pickDir()}>
            选择目录
          </button>
        </div>
      </label>
      <div className="skill-dialog-foot">
        <button type="button" className="btn" disabled={busy} onClick={() => void submit()}>
          {busy ? "导入中…" : "导入"}
        </button>
      </div>
    </>
  );
}
