"use client";

// Composer 内模型短名 + 切换菜单（plan-chat-first-ui v2 Q3）。
import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { useLlmStore } from "@/lib/stores/llmStore";
import { settingsHref } from "@/lib/settingsSections";

function shortLabel(
  loaded: boolean,
  provider: string,
  localModel: string,
  activeId: string,
  cloudEntries: { id: string; name: string }[]
): string {
  if (!loaded) return "连接中…";
  if (provider === "cloud") {
    const entry = cloudEntries.find((e) => `cloud:${e.id}` === activeId);
    return entry?.name ?? "未选择";
  }
  // ollama:qwen3.5:2b → 取 tag 末段缩短
  const tag = localModel || activeId.replace(/^ollama:/, "");
  if (tag.length <= 14) return tag;
  return `${tag.slice(0, 12)}…`;
}

export function ModelChip() {
  const router = useRouter();
  const loaded = useLlmStore((s) => s.loaded);
  const provider = useLlmStore((s) => s.provider);
  const activeId = useLlmStore((s) => s.activeId);
  const localModel = useLlmStore((s) => s.localModel);
  const installed = useLlmStore((s) => s.installed);
  const cloudEntries = useLlmStore((s) => s.cloudEntries);
  const testingId = useLlmStore((s) => s.testingId);
  const activateId = useLlmStore((s) => s.activateId);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const label = shortLabel(loaded, provider, localModel, activeId, cloudEntries);
  const busy = !!testingId;

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  async function pick(id: string) {
    setOpen(false);
    if (id === activeId || busy) return;
    await activateId(id);
  }

  return (
    <div className={`model-chip-wrap${open ? " open" : ""}`} ref={rootRef}>
      <button
        type="button"
        className="model-chip"
        aria-label="切换模型"
        aria-expanded={open}
        aria-haspopup="menu"
        disabled={busy}
        title={busy ? "正在切换…" : "切换模型"}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="model-chip-label">{busy ? "切换中…" : label}</span>
        <svg viewBox="0 0 12 12" aria-hidden="true">
          <polyline points="2 4 6 8 10 4" />
        </svg>
      </button>
      {open && (
        <div className="model-chip-menu" role="menu">
          {installed.map((m) => {
            const id = `ollama:${m.name}`;
            return (
              <button
                key={id}
                type="button"
                role="menuitemradio"
                aria-checked={activeId === id}
                className={`model-chip-item${activeId === id ? " on" : ""}`}
                onClick={() => void pick(id)}
              >
                {m.name}
              </button>
            );
          })}
          {cloudEntries.map((e) => {
            const id = `cloud:${e.id}`;
            return (
              <button
                key={id}
                type="button"
                role="menuitemradio"
                aria-checked={activeId === id}
                className={`model-chip-item${activeId === id ? " on" : ""}`}
                onClick={() => void pick(id)}
              >
                云端 · {e.name}
              </button>
            );
          })}
          {(installed.length > 0 || cloudEntries.length > 0) && (
            <div className="model-chip-sep" role="separator" />
          )}
          <button
            type="button"
            role="menuitem"
            className="model-chip-item manage"
            onClick={() => {
              setOpen(false);
              router.push(settingsHref("model"));
            }}
          >
            管理模型…
          </button>
        </div>
      )}
    </div>
  );
}
