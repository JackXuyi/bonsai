"use client";

// 远端加载表单：GitHub 仓库/目录/raw、GitLab blob、任意 https SKILL.md。
// 联网只在点「加载」时发生，没有后台轮询。
import { useState } from "react";
import { humanizeAgentError, skillLoadRemote } from "@/lib/commands";

export function SkillRemoteForm({ onDone }: { onDone: () => void }) {
  const [url, setUrl] = useState("");
  const [id, setId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    if (!url.trim()) {
      setError("先贴一个链接");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await skillLoadRemote(url.trim(), id.trim() || undefined);
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
        支持 GitHub 仓库（<code>github.com/owner/repo</code>）、子目录
        （<code>…/tree/main/skills/gmp</code>）、<code>raw.githubusercontent.com</code> 的
        SKILL.md 链接，以及任意以 <code>.md</code> 结尾的 https 链接（单文件，不带引用）。
        <br />
        只在你点「加载」时联网；内容按外部来源处理（注入黑名单 + untrusted 标注 + 大小上限：
        SKILL.md ≤16KB、references ≤4KB×5，超出会明确报错不落盘）。
      </p>
      {error && <p className="model-error">{error}</p>}
      <label className="agent-field">
        链接
        <input
          data-testid="skill-remote-url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://github.com/owner/repo/tree/main/skills/gmp"
        />
      </label>
      <label className="agent-field">
        id（可选，留空从链接推导）
        <input
          data-testid="skill-remote-id"
          value={id}
          onChange={(e) => setId(e.target.value)}
          placeholder="gmp-assistant"
        />
      </label>
      <div className="skill-dialog-foot">
        <button
          type="button"
          className="btn"
          data-testid="skill-remote-submit"
          disabled={busy}
          onClick={() => void submit()}
        >
          {busy ? "加载中…" : "加载"}
        </button>
      </div>
    </>
  );
}
