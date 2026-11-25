"use client";

// 设置 · 形象：可配置称呼（plan-presence-im v4）。
import { useEffect, useState } from "react";
import { usePersonaStore } from "@/lib/stores/personaStore";
import { useUiStore } from "@/lib/stores/uiStore";

export function PersonaNameField() {
  const name = usePersonaStore((s) => s.name);
  const error = usePersonaStore((s) => s.error);
  const ensureLoaded = usePersonaStore((s) => s.ensureLoaded);
  const setName = usePersonaStore((s) => s.setName);
  const [draft, setDraft] = useState(name);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    ensureLoaded();
  }, [ensureLoaded]);

  useEffect(() => {
    setDraft(name);
  }, [name]);

  async function save() {
    if (busy) return;
    const next = draft.trim();
    if (next === name) return;
    setBusy(true);
    try {
      await setName(next);
      useUiStore.getState().pushToast(`称呼已改为「${next}」`);
    } catch {
      // error 已写入 store
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card persona-name-card" data-testid="persona-name-field">
      <h3 className="card-title">称呼</h3>
      <p className="hint-line">空态标题与头像悬停会用这个名字（1–12 个字）。</p>
      <div className="persona-name-row">
        <input
          className="persona-name-input"
          data-testid="persona-name-input"
          value={draft}
          maxLength={12}
          disabled={busy}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void save();
          }}
          aria-label="称呼"
        />
        <button
          type="button"
          className="persona-name-save"
          data-testid="persona-name-save"
          disabled={busy || draft.trim() === name || !draft.trim()}
          onClick={() => void save()}
        >
          保存
        </button>
      </div>
      {error ? (
        <p className="hint-line persona-name-error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
