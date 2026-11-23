"use client";

// 技能预览 dialog（skill_read）：全文 + 引用，markdown 渲染。
// 复用日报 dialog 的 <dialog> + renderMarkdown 骨架（esc 先转义，无注入面扩大）。
import { useEffect, useRef, useState } from "react";
import { humanizeAgentError, skillRead } from "@/lib/commands";
import { renderMarkdown } from "@/lib/markdown";
import type { SkillDetail } from "@/lib/types";

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
    skillRead(id)
      .then(setDetail)
      .catch((e) => setError(humanizeAgentError(e)));
  }, [open, id]);

  return (
    <dialog
      ref={ref}
      className="skill-dialog"
      data-testid="skill-preview-dialog"
      onClick={(e) => {
        if (e.target === ref.current) onClose();
      }}
      onClose={onClose}
    >
      <div className="dialog-head">
        <span>技能预览</span>
        <button aria-label="关闭" onClick={onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
      {error && <p className="model-error">{error}</p>}
      {detail && (
        <div className="dialog-body" data-testid="skill-preview-body">
          <p className="skill-note">
            {detail.name} · {detail.enabled ? "已启用" : "未启用"} · 注入预算{" "}
            {detail.sizeBytes} 字节
            <br />
            {detail.description}
          </p>
          <div dangerouslySetInnerHTML={{ __html: renderMarkdown(detail.body) }} />
          {detail.refs.map((r) => (
            <section key={r.name}>
              <p className="card-title">引用：{r.name}</p>
              <div dangerouslySetInnerHTML={{ __html: renderMarkdown(r.content) }} />
            </section>
          ))}
        </div>
      )}
    </dialog>
  );
}
