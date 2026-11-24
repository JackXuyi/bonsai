"use client";

// 技能预览页（skill_read）：通用文档布局——名称 / 版本 / 描述等元信息 +
// markdown 正文与引用。用 MarkdownText（GFM），不用旧 renderMarkdown 子集。
import { useEffect, useRef, useState } from "react";
import { humanizeAgentError, skillRead } from "@/lib/commands";
import { MarkdownText } from "@/components/chat/MarkdownText";
import type { SkillDetail } from "@/lib/types";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function SkillPreviewDialog({
  open,
  id,
  onClose,
}: {
  open: boolean;
  id: string | null;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) el.showModal();
    if (!open && el.open) el.close();
  }, [open]);

  useEffect(() => {
    if (!open || !id) return;
    setDetail(null);
    setError(null);
    setLoading(true);
    skillRead(id)
      .then(setDetail)
      .catch((e) => setError(humanizeAgentError(e)))
      .finally(() => setLoading(false));
  }, [open, id]);

  return (
    <dialog
      ref={ref}
      className="skill-dialog skill-preview-dialog"
      data-testid="skill-preview-dialog"
      onClick={(e) => {
        if (e.target === ref.current) onClose();
      }}
      onClose={onClose}
    >
      <div className="dialog-head">
        <span>技能详情</span>
        <button type="button" aria-label="关闭" onClick={onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
      {error && <p className="model-error">{error}</p>}
      {loading && !detail && !error && (
        <p className="hint-line skill-preview-loading">读取中…</p>
      )}
      {detail && (
        <div className="dialog-body skill-preview" data-testid="skill-preview-body">
          <header className="skill-preview-hero">
            <h1 className="skill-preview-title">{detail.name}</h1>
            <p className="skill-preview-id">
              <code>{detail.id}</code>
            </p>
          </header>

          <dl className="skill-preview-meta" data-testid="skill-preview-meta">
            <div className="skill-preview-meta-row">
              <dt>名称</dt>
              <dd>{detail.name}</dd>
            </div>
            <div className="skill-preview-meta-row">
              <dt>版本</dt>
              <dd data-testid="skill-preview-version">
                {detail.version?.trim() ? detail.version : "—"}
              </dd>
            </div>
            <div className="skill-preview-meta-row">
              <dt>状态</dt>
              <dd>{detail.enabled ? "已启用" : "未启用"}</dd>
            </div>
            <div className="skill-preview-meta-row">
              <dt>体积</dt>
              <dd>{formatBytes(detail.sizeBytes)}</dd>
            </div>
            <div className="skill-preview-meta-row skill-preview-meta-desc">
              <dt>描述</dt>
              <dd>{detail.description}</dd>
            </div>
          </dl>

          <section className="skill-preview-section" aria-label="正文">
            <h2 className="skill-preview-section-title">正文</h2>
            <div className="skill-preview-doc" data-testid="skill-preview-markdown">
              <MarkdownText text={detail.body} />
            </div>
          </section>

          {detail.refs.map((r) => (
            <section
              key={r.name}
              className="skill-preview-section"
              aria-label={`引用 ${r.name}`}
            >
              <h2 className="skill-preview-section-title">引用 · {r.name}</h2>
              <div className="skill-preview-doc">
                <MarkdownText text={r.content} />
              </div>
            </section>
          ))}
        </div>
      )}
    </dialog>
  );
}
