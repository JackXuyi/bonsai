"use client";

// 日报 dialog（旧 reportDialog 的 <dialog> + renderMarkdown 渲染）。
import { useEffect, useRef } from "react";
import { renderMarkdown } from "@/lib/markdown";

export function ReportDialog({ open, md, onClose }: { open: boolean; md: string | null; onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) el.showModal();
    if (!open && el.open) el.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      onClick={(e) => {
        // click on the backdrop dismisses (dialog padding is 0)
        if (e.target === ref.current) onClose();
      }}
      onClose={onClose}
    >
      <div className="dialog-head">
        <span>日报</span>
        <button aria-label="关闭" onClick={onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
      <div
        className="report-body"
        dangerouslySetInnerHTML={{ __html: md ? renderMarkdown(md) : "" }}
      />
    </dialog>
  );
}
