"use client";

// 添加技能 dialog 外壳：三个入口页签，表单各自持有状态。
// 目录导入（本地）/ 手写创建 / 从远端加载（GitHub 等）。
import { useEffect, useRef, useState } from "react";
import { SkillCreateForm } from "./SkillCreateForm";
import { SkillImportForm } from "./SkillImportForm";
import { SkillRemoteForm } from "./SkillRemoteForm";

type Mode = "import" | "create" | "remote";

const TABS: { key: Mode; label: string }[] = [
  { key: "import", label: "目录导入" },
  { key: "create", label: "手写创建" },
  { key: "remote", label: "从远端加载" },
];

export function SkillAddDialog({
  open,
  onClose,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const [mode, setMode] = useState<Mode>("import");

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) {
      setMode("import");
      el.showModal();
    }
    if (!open && el.open) el.close();
  }, [open]);

  function done() {
    onChanged();
    onClose();
  }

  return (
    <dialog
      ref={ref}
      className="skill-dialog"
      data-testid="skill-add-dialog"
      onClick={(e) => {
        if (e.target === ref.current) onClose();
      }}
      onClose={onClose}
    >
      <div className="dialog-head">
        <span>添加技能</span>
        <button aria-label="关闭" onClick={onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
      <div className="dialog-body">
        <div className="skill-tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t.key}
              type="button"
              role="tab"
              aria-selected={mode === t.key}
              className="skill-tab"
              data-active={mode === t.key}
              onClick={() => setMode(t.key)}
            >
              {t.label}
            </button>
          ))}
        </div>
        {/* 关闭即卸载表单：下次打开不残留上次的链接/报错 */}
        {!open ? null : mode === "import" ? (
          <SkillImportForm onDone={done} />
        ) : mode === "create" ? (
          <SkillCreateForm onDone={done} />
        ) : (
          <SkillRemoteForm onDone={done} />
        )}
      </div>
    </dialog>
  );
}
