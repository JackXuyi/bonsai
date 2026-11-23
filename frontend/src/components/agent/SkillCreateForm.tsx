"use client";

// 手写创建表单：四项必填，后端拼 frontmatter 后再跑同一套校验。
import { useState } from "react";
import { humanizeAgentError, skillCreate } from "@/lib/commands";

export function SkillCreateForm({ onDone }: { onDone: () => void }) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    if (!id.trim() || !name.trim() || !description.trim() || !body.trim()) {
      setError("四项都要填：id / 名称 / 描述 / 正文");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await skillCreate({
        id: id.trim(),
        name: name.trim(),
        description: description.trim(),
        body,
      });
      onDone();
    } catch (e) {
      setError(humanizeAgentError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      {error && <p className="model-error">{error}</p>}
      <label className="agent-field">
        id（小写字母/数字/连字符）
        <input value={id} onChange={(e) => setId(e.target.value)} placeholder="my-skill" />
      </label>
      <label className="agent-field">
        名称
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="GMP 助手" />
      </label>
      <label className="agent-field">
        描述（作为一级披露展示）
        <input
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="一句话说清这个技能做什么"
        />
      </label>
      <label className="agent-field">
        正文（SKILL.md body，≤16KB）
        <textarea
          data-testid="skill-create-body"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="注入给模型的提示段内容"
        />
      </label>
      <div className="skill-dialog-foot">
        <button type="button" className="btn" disabled={busy} onClick={() => void submit()}>
          {busy ? "创建中…" : "创建"}
        </button>
      </div>
    </>
  );
}
